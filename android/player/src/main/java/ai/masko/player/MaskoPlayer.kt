package ai.masko.player

import ai.masko.player.engine.EngineClipKind
import ai.masko.player.engine.EngineCommand
import ai.masko.player.engine.LoadFailure
import ai.masko.player.engine.MaskoEngine
import android.media.MediaCodecList
import android.os.Build
import org.json.JSONObject
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.random.Random

/**
 * Plays one Masko mascot release. Apps call [set] and [fire]; the canvas,
 * through the shared engine, decides which clip plays (spec/player.md).
 *
 * Make one with [MaskoPlayerView.load]. Call every method on the main thread:
 * a call from another thread throws [IllegalStateException]. Events reach
 * listeners on the main thread, starting on the next main-thread turn, so
 * listeners added right after loading hear the first pose.
 */
class MaskoPlayer internal constructor(
    /** The release delivery; ignored when [clip] is given. */
    deliveryJson: String,
    private val stage: Stage,
    private val sounds: Sounds,
    private val scheduler: Scheduler,
    elapsedMs: () -> Long,
    private val wallClockMs: () -> Long,
    private val options: Options,
    /** The engine's random source for extra clips (rules.md 7.3): its own for every player. */
    private val seed: ULong = randomSeed(),
    private val makeEngine: (String) -> MaskoEngine = ::MaskoEngine,
    /**
     * One animation's stacked file, played on its own in a loop in place of
     * the delivery: its delivery waits for the file's length (player.md, "One clip").
     */
    clip: String? = null,
) {
    /** How the player starts and refreshes its links. */
    class Options(
        /**
         * False shows the first pose's first frame and waits for [play]. Null
         * (the default) follows the graph's `autoPlay`, else true; the app's
         * value wins over the graph's.
         */
        val autoPlay: Boolean? = null,
        /** Whether clips' own voices are heard at first (see [sound]); on by default. */
        val sound: Boolean = true,
        /** Gives a fresh delivery of the same release before the signed links expire. */
        val getDelivery: DeliverySource? = null,
    )

    /** The app's way to fetch the release's delivery again. */
    fun interface DeliverySource {
        /**
         * Fetch the delivery and call [reply] once, from any thread, with its
         * JSON or null. Called on the main thread. An exception thrown here, or
         * no reply within 60 seconds, is a failed refresh (`refresh_failed`),
         * asked again 30 seconds later; a reply after that is ignored.
         */
        fun getDelivery(reply: (String?) -> Unit)
    }

    /** What the app hears (spec/player.md, "What the app sees"). Every method has an empty default. */
    interface Listener {
        /** The mascot is on this pose now. */
        fun onPose(pose: String) {}

        /**
         * This edge's clip is on screen now, or `still:<pose>` for a pose's
         * still image, with its [kind] and its playback [rate].
         */
        fun onClip(clip: String, kind: ClipKind, rate: Double) {}

        /** A fire is done: [used] says whether a start used it (rules.md 3.3). */
        fun onReactionDone(name: String, used: Boolean) {}

        /**
         * A move's clip is on screen; [movement] is the edge's `movement`, for
         * the app to move the mascot over [seconds] (the edge's duration at [rate]).
         */
        fun onMovement(edge: String, movement: JSONObject, rate: Double, seconds: Double) {}

        /** Something went wrong; [code] comes from rules.md or the player (README), [detail] says more when it can. */
        fun onError(code: String, detail: String?) {}
    }

    /** What an [Listener.onClip] shows: a pose clip, a move, or a pose's still. */
    enum class ClipKind { POSE, MOVE, STILL }

    /** The clip on screen and its clip time in seconds. */
    data class Position(val clip: String?, val time: Double)

    /** For editors and tools only (rules.md 13). */
    inner class Preview internal constructor() {
        /** Cuts to [pose] at once. */
        fun showPose(pose: String) = appCall("preview.showPose") { engine.previewShowPose(pose, it) }

        /** Plays one move from the current pose at once. */
        fun playMove(edge: String) = appCall("preview.playMove") { engine.previewPlayMove(edge, it) }
    }

    val preview = Preview()

    /**
     * Whether clips' own voices (`hasAudio`) are heard. On by default; false
     * mutes them. Edge sounds play either way (rules.md 9.6).
     */
    var sound: Boolean = options.sound
        set(value) {
            onMain("sound")
            field = value
            want?.let { slots[it.slot].setVolume(voice(it.edge)) }
            onScreen?.let { slots[it.slot].setVolume(voice(it.edge)) }
        }

    private val listeners = CopyOnWriteArrayList<Listener>()
    private lateinit var engine: MaskoEngine
    private var delivery = Delivery.NONE
    private val clock = PlayClock(elapsedMs)
    private val slots = stage.slots

    private var started = false
    private var destroyed = false
    private var appPaused = !(options.autoPlay ?: true)
    private var hostPaused = false
    private val paused get() = appPaused || hostPaused
    /** Whether the view is in a window; a detached view keeps nothing queued on the main thread. */
    private var attached = true

    /** App calls made while paused, decided on when play resumes (rules.md 11.2). */
    private val waiting = ArrayDeque<(ULong) -> List<EngineCommand>>()
    private val commands = ArrayDeque<EngineCommand>()
    private var running = false
    private val unplayable = HashSet<String>()

    /** A clip shown in a slot. */
    private class Showing(val slot: Int, val edge: String, val kind: EngineClipKind, val rate: Double) {
        var ended = false
    }

    /** What each slot holds: the edge, its file, whether its first frame is drawn, whether it is still on it. */
    private class Held(var edge: String? = null, var file: String? = null, var ready: Boolean = false, var fresh: Boolean = false)

    private val held = Array(slots.size) { Held() }

    /** The clip the engine last asked for; null while a still is asked for. */
    private var want: Showing? = null

    /** The clip on screen. It differs from [want] while the next clip loads. */
    private var onScreen: Showing? = null

    /** A clip just put on screen, reported shown once the commands being carried out are done. */
    private var toReport: Showing? = null

    /** The slot last made visible. */
    private var front = -1
    private var stillOnScreen: String? = null
    private var stillLoading = false
    private var stillToken = 0

    private val wakeTask = Runnable { call { engine.advance(it) } }
    private val refreshTask = Runnable { refresh() }
    private val replyTimeout = Runnable { if (asking) refreshFailed("getDelivery did not reply within 60 seconds") }

    /** When the next refresh is due, on the wall clock; null while none is (or while one is asked). */
    private var refreshAt: Long? = null
    /** Whether getDelivery was asked and has not replied. */
    private var asking = false
    /** Counts the asks: a reply to an older one is dropped. */
    private var asks = 0

    init {
        slots.forEachIndexed { i, slot -> slot.events = SlotEvents(i) }
        if (clip == null) {
            open(deliveryJson)
            appPaused = !(options.autoPlay ?: delivery.autoPlay)
        } else {
            stage.clipFile(clip) { file ->
                when {
                    destroyed -> Unit
                    file == null -> fail("clip_failed", "cannot read the length of $clip")
                    else -> open(Delivery.clip(clip, file.seconds, file.voiced))
                }
            }
        }
    }

    /** Starts the engine on [json]; a delivery it refuses plays nothing (rules.md 1.2). */
    private fun open(json: String) {
        var refusal: Pair<String, String?>? = null
        try {
            engine = makeEngine(json)
        } catch (e: LoadFailure.Refused) {
            // The engine's own reason, as on every platform: `unreadable` for text that is not JSON.
            refusal = e.code to e.detail
        } catch (e: Exception) {
            // `engine_failed` is only for an engine that cannot run: here it threw instead of refusing.
            refusal = "engine_failed" to e.message
        } catch (e: LinkageError) {
            // Or no engine library for this device's ABI.
            refusal = "engine_failed" to e.message
        }
        if (refusal != null) {
            fail(refusal.first, refusal.second)
            return
        }
        delivery = Delivery.parse(json) ?: delivery
        try {
            engine.seed(seed)
        } catch (e: Exception) {
            scheduler.post { emit { it.onError("engine_failed", e.message) } }
        }
        stage.whenReady(
            ready = { scheduler.post { begin() } },
            failed = { detail ->
                scheduler.post { if (!destroyed) emit { it.onError("surface_unavailable", detail) } }
            },
        )
        scheduleRefresh()
    }

    /** Plays nothing, and frees the decoders at once: the delivery or the clip cannot play. */
    private fun fail(code: String, detail: String?) {
        destroyed = true
        stage.release()
        sounds.release()
        scheduler.post { emit { it.onError(code, detail) } }
    }

    // ------------------------------------------------------------------ app API

    fun addListener(listener: Listener) {
        listeners += listener
    }

    fun removeListener(listener: Listener) {
        listeners -= listener
    }

    /**
     * Holds a value until the next `set` of that name (rules.md 3.2). With
     * [instant], what it starts happens now: an Any State start goes straight
     * into its target pose, skipping every transition clip and route, and a
     * move plays its own clip (rules.md 5.8).
     */
    @JvmOverloads
    fun set(name: String, value: Boolean, instant: Boolean = false) =
        appCall("set") { engine.setBool(name, value, instant, it) }

    @JvmOverloads
    fun set(name: String, value: Double, instant: Boolean = false) =
        appCall("set") { engine.setNumber(name, value, instant, it) }

    @JvmOverloads
    fun set(name: String, value: Int, instant: Boolean = false) = set(name, value.toDouble(), instant)

    /** Makes the name read true until a clip uses it (rules.md 3.3), with [instant] as for [set]. */
    @JvmOverloads
    fun fire(name: String, instant: Boolean = false) = appCall("fire") { engine.fire(name, instant, it) }

    /** Values go back to their defaults and the mascot starts again (rules.md 11.3). */
    fun reset() = appCall("reset") { engine.reset(it) }

    /** Starts the mascot, or resumes it exactly where it stopped. */
    fun play() {
        onMain("play")
        appPaused = false
        applyPause()
    }

    /** Stops play time, clips, timers and sounds. */
    fun pause() {
        onMain("pause")
        appPaused = true
        applyPause()
    }

    val isPaused: Boolean get() = paused

    /** The clip on screen and its clip time, read only; `clip` is null before anything shows. */
    fun position(): Position {
        onMain("position")
        onScreen?.let { return Position(it.edge, slots[it.slot].position()) }
        return Position(stillOnScreen?.let { "still:$it" }, 0.0)
    }

    /**
     * Takes a fresh delivery of the same release, as [Options.getDelivery]
     * gives one: only its links are used, the engine keeps running, cached
     * files keep playing and clips that failed are tried again. Another
     * release is refused with `refresh_mismatch`, a delivery that cannot be
     * read with `refresh_failed`.
     */
    fun updateDelivery(json: String): Boolean {
        onMain("updateDelivery")
        if (destroyed) return false
        val fresh = Delivery.parse(json)
        if (fresh == null) {
            emit { it.onError("refresh_failed", "not a delivery") }
            return false
        }
        if (!delivery.sameRelease(fresh)) {
            emit { it.onError("refresh_mismatch", "getDelivery returned another release") }
            return false
        }
        delivery = fresh
        // Clips that failed may have failed on an expired link.
        unplayable.toList().forEach(::preload)
        scheduleRefresh()
        return true
    }

    /**
     * Stops everything and frees the decoders, preloads and timers. The
     * player does nothing afterwards.
     */
    fun destroy() {
        onMain("destroy")
        val wasRunning = !destroyed
        destroyed = true
        scheduler.cancel(wakeTask)
        scheduler.cancel(refreshTask)
        scheduler.cancel(replyTimeout)
        waiting.clear()
        commands.clear()
        stage.release()
        sounds.release()
        if (wasRunning && ::engine.isInitialized) {
            try {
                engine.destroy()
            } catch (_: Exception) {
                // Freed either way.
            }
        }
        listeners.clear()
    }

    /** The view is on screen or not: off screen, the player pauses itself. */
    internal fun setHostActive(active: Boolean) {
        if (destroyed) return
        val back = active && hostPaused
        hostPaused = !active
        applyPause()
        if (back) catchUpRefresh()
    }

    /**
     * The view entered or left its window. Detached, nothing stays queued on
     * the main thread and the sounds' decoders are freed, so a player the app
     * forgot to destroy holds nothing alive; back in a window, it catches up.
     */
    internal fun setHostAttached(attached: Boolean) {
        if (destroyed || attached == this.attached) return
        this.attached = attached
        if (attached) {
            catchUpRefresh()
        } else {
            scheduler.cancel(refreshTask)
            if (asking) {
                // A reply that comes while detached is dropped; back in a window, the player asks again at once.
                asking = false
                asks++
                scheduler.cancel(replyTimeout)
                refreshAt = wallClockMs()
            }
            sounds.detach()
        }
    }

    // ------------------------------------------------------------------ engine calls

    private fun begin() {
        if (destroyed || started) return
        started = true
        call { engine.start(it) }
        applyPause()
    }

    private fun appCall(method: String, f: (ULong) -> List<EngineCommand>) {
        onMain(method)
        if (destroyed) return
        if (started && paused) waiting.addLast(f) else call(f)
    }

    private fun onMain(method: String) {
        check(scheduler.isMainThread()) {
            "MaskoPlayer.$method must be called on the main thread, not on thread \"${Thread.currentThread().name}\""
        }
    }

    /**
     * An engine call. An exception (a Rust panic unwinds into one) is the
     * other `engine_failed`, reported instead of thrown; the call gives [fallback].
     */
    private inline fun <T> engineCall(fallback: T, f: () -> T): T = try {
        f()
    } catch (e: Exception) {
        emit { it.onError("engine_failed", e.message) }
        fallback
    }

    /** Calls the engine at the current play time and carries out its commands in order. */
    private fun call(f: (ULong) -> List<EngineCommand>) {
        // A lone clip drops calls until its engine starts.
        if (destroyed || !::engine.isInitialized) return
        commands.addAll(engineCall(emptyList()) { f(clock.now().toULong()) })
        run()
    }

    /**
     * Carries out the queued commands in order. A clip they put on screen is
     * reported shown after them, unless a later one replaced it: once per
     * showing, never for one the engine has moved past (player.md, clipShown).
     */
    private fun run() {
        if (running || destroyed) return
        running = true
        try {
            while (!destroyed) {
                val next = commands.removeFirstOrNull()
                if (next != null) {
                    handle(next)
                    continue
                }
                val shown = toReport ?: break
                toReport = null
                if (shown === onScreen && shown === want) call { engine.clipShown(shown.edge, it) }
            }
        } finally {
            running = false
        }
        if (destroyed) return
        warm()
        scheduleWake()
    }

    private fun handle(c: EngineCommand) {
        when (c) {
            is EngineCommand.ShowClip -> showClip(c.edge, c.kind, c.rate)
            is EngineCommand.ShowStill -> showStill(c.pose)
            is EngineCommand.Preload -> preload(c.edge)
            is EngineCommand.PlaySound -> delivery.edges[c.edge]?.sound?.let { sounds.play(it.url, it.volume, it.loop) }
            is EngineCommand.StopSounds -> sounds.stop(c.onlyLooping)
            is EngineCommand.PoseEntered -> emit { it.onPose(c.pose) }
            // The next showClip replaces the move on screen at once; nothing else to do.
            is EngineCommand.Interrupted -> Unit
            is EngineCommand.Spent -> emit { it.onReactionDone(c.name, c.used) }
            is EngineCommand.Error -> {
                // The clip asked for was not on screen in time: the engine skips it until it loads (rules.md 9.3).
                if (c.code == "clip_timeout") want?.let { unplayable += it.edge }
                emit { it.onError(c.code, c.detail) }
            }
        }
    }

    private fun scheduleWake() {
        scheduler.cancel(wakeTask)
        if (!started || paused || destroyed) return
        val at = engineCall(null) { engine.nextWake() }?.toLong() ?: return
        scheduler.postDelayed(wakeTask, (at - clock.now()).coerceAtLeast(0))
    }

    private fun applyPause() {
        if (!started || destroyed) return
        val run = !paused
        if (run == clock.running) return
        if (run) {
            clock.resume()
            onScreen?.let { if (!it.ended) slots[it.slot].play(it.rate) }
            sounds.resume()
            while (!paused && !destroyed) call(waiting.removeFirstOrNull() ?: break)
            scheduleWake()
        } else {
            clock.pause()
            scheduler.cancel(wakeTask)
            onScreen?.let { slots[it.slot].pause() }
            sounds.pause()
        }
    }

    // ------------------------------------------------------------------ commands

    private fun showClip(edge: String, kind: EngineClipKind, rate: Double) {
        if (stillLoading) {
            stillLoading = false
            stillToken++
            stage.cancelStill()
        }
        val url = delivery.edges[edge]?.video(stage.sizePx())
        if (url == null) {
            // No stacked file for this edge: it cannot play here (rules.md 9.4, 9.5).
            want = null
            unplayable += edge
            call { engine.clipFailed(edge, it) }
            return
        }
        val current = onScreen
        if (kind == EngineClipKind.POSE && current != null && current.edge == edge) {
            // The next loop of the pose clip on screen: restart it in place.
            val next = Showing(current.slot, edge, kind, rate)
            want = next
            onScreen = next
            val slot = slots[current.slot]
            slot.setVolume(voice(edge))
            slot.rewind()
            if (!paused) slot.play(rate)
            emit { it.onClip(edge, ClipKind.POSE, rate) }
            // Restarted in place, it is on screen at once.
            toReport = next
            return
        }
        val target = want?.takeIf { it !== onScreen }?.slot ?: (if (front < 0) 0 else 1 - front)
        val next = Showing(target, edge, kind, rate)
        want = next
        val h = held[target]
        if (h.edge == edge && h.file == fileKey(url) && h.fresh) {
            if (h.ready) reveal()
            return
        }
        hold(target, edge, url)
    }

    /** Loads [url] paused on its first frame in [slot]. */
    private fun hold(slot: Int, edge: String, url: String) {
        held[slot].apply {
            this.edge = edge
            file = fileKey(url)
            ready = false
            fresh = true
        }
        slots[slot].load(url)
    }

    /** Puts the clip that was loading on screen, hiding the old one in the same frame. */
    private fun reveal() {
        val next = want ?: return
        if (next === onScreen) return
        val slot = slots[next.slot]
        slot.setVolume(voice(next.edge))
        slot.setVisible(true)
        slots.forEachIndexed { i, other -> if (i != next.slot) other.setVisible(false) }
        onScreen?.let { old -> if (old.slot != next.slot) slots[old.slot].pause() }
        stage.hideStill()
        stillOnScreen = null
        front = next.slot
        onScreen = next
        held[next.slot].fresh = false
        if (!paused) slot.play(next.rate)
        val move = next.kind == EngineClipKind.MOVE
        emit { it.onClip(next.edge, if (move) ClipKind.MOVE else ClipKind.POSE, next.rate) }
        val edge = delivery.edges[next.edge]
        if (move && edge?.movement != null) {
            emit { it.onMovement(next.edge, edge.movement, next.rate, edge.duration / next.rate) }
        }
        toReport = next
    }

    private fun showStill(pose: String) {
        want = null
        stillLoading = true
        val token = ++stillToken
        stage.showStill(delivery.stills[pose]) {
            if (token != stillToken || destroyed) return@showStill
            stillLoading = false
            onScreen?.let {
                slots[it.slot].setVisible(false)
                slots[it.slot].pause()
            }
            onScreen = null
            stillOnScreen = pose
            emit { it.onClip("still:$pose", ClipKind.STILL, 1.0) }
        }
    }

    private fun preload(edge: String) {
        val url = delivery.edges[edge]?.video(stage.sizePx()) ?: return
        stage.preload(url) { ok -> if (!destroyed) playable(edge, ok) }
    }

    /** A preload or a warm-up says whether [edge] can load (player.md: markUnplayable, markPlayable). */
    private fun playable(edge: String, ok: Boolean) {
        if (!ok) {
            if (unplayable.add(edge)) engineCall(Unit) { engine.markUnplayable(edge) }
        } else if (unplayable.remove(edge)) {
            engineCall(Unit) { engine.markPlayable(edge) }
        }
    }

    /**
     * Prepares the next clip in the hidden slot before it is asked for: the
     * decided start's first clip, or during a move the target's main clip.
     */
    private fun warm() {
        if (!started || want !== onScreen) return
        val state = engineCall(null) { engine.state() } ?: return
        val current = onScreen
        val edge = state.nextClip
            ?: current?.takeIf { it.kind == EngineClipKind.MOVE }?.let { m ->
                delivery.edges[m.edge]?.target?.let(delivery::mainClip)
            }
            ?: return
        if (edge == current?.edge || edge in unplayable) return
        val url = delivery.edges[edge]?.video(stage.sizePx()) ?: return
        val slot = if (front < 0) 0 else 1 - front
        val h = held[slot]
        if (h.edge == edge && h.file == fileKey(url) && h.fresh) return
        hold(slot, edge, url)
    }

    private fun voice(edge: String): Float =
        if (sound && delivery.edges[edge]?.hasAudio == true) 1f else 0f

    private inline fun emit(event: (Listener) -> Unit) {
        for (l in listeners) event(l)
    }

    private inner class SlotEvents(private val index: Int) : ClipSlot.Events {
        override fun onFirstFrame() {
            if (destroyed) return
            val h = held[index]
            h.ready = true
            h.edge?.let { playable(it, true) }
            val next = want
            if (next != null && next !== onScreen && next.slot == index) {
                reveal()
                // Reports it shown, then warms the other slot.
                run()
            }
        }

        override fun onEnded() {
            if (destroyed) return
            val shown = onScreen ?: return
            if (shown.slot != index || shown !== want || shown.ended) return
            shown.ended = true
            call { engine.clipEnded(shown.edge, it) }
        }

        override fun onFailed(message: String) {
            if (destroyed) return
            val h = held[index]
            val edge = h.edge
            val warming = h.fresh
            h.edge = null
            h.ready = false
            h.fresh = false
            val asked = want
            if (asked != null && asked.slot == index && edge == asked.edge) {
                // The clip on screen, or the one loading for it, cannot play.
                want = null
                unplayable += asked.edge
                call { engine.clipFailed(asked.edge, it) }
            } else if (edge != null && warming) {
                // A warm-up failed: the engine skips that clip until it loads.
                playable(edge, false)
            }
        }
    }

    // ------------------------------------------------------------------ signed links

    private fun scheduleRefresh() {
        if (destroyed || options.getDelivery == null) return
        val expires = delivery.expiresAt
        if (expires == null) {
            scheduler.cancel(refreshTask)
            refreshAt = null
            return
        }
        val left = expires - wallClockMs()
        refreshIn(if (left > 2 * REFRESH_LEAD_MS) left - REFRESH_LEAD_MS else left / 2)
    }

    private fun refreshIn(delayMs: Long) {
        scheduler.cancel(refreshTask)
        val delay = delayMs.coerceAtLeast(0)
        refreshAt = wallClockMs() + delay
        // Detached, only the time is kept: catchUpRefresh schedules it when the view is back.
        if (attached) scheduler.postDelayed(refreshTask, delay)
    }

    /**
     * The refresh timer counts uptime, which stops in deep sleep: when the
     * view is back, the time left is read again from the wall clock, and a
     * refresh already due starts at once.
     */
    private fun catchUpRefresh() {
        val at = refreshAt ?: return
        if (destroyed || asking) return
        refreshIn(at - wallClockMs())
    }

    private fun refresh() {
        val source = options.getDelivery ?: return
        if (destroyed || asking) return
        refreshAt = null
        asking = true
        val ask = ++asks
        scheduler.postDelayed(replyTimeout, REPLY_TIMEOUT_MS)
        try {
            source.getDelivery { json ->
                scheduler.post {
                    if (ask == asks && asking && !destroyed) {
                        asking = false
                        scheduler.cancel(replyTimeout)
                        applyRefresh(json)
                    }
                }
            }
        } catch (e: Exception) {
            // The app's getDelivery threw: a failed refresh, not a crash.
            refreshFailed(e.message ?: e.toString())
        }
    }

    /** getDelivery threw or never replied: reported, and asked again later; a late reply is dropped. */
    private fun refreshFailed(detail: String) {
        if (destroyed) return
        asking = false
        asks++
        scheduler.cancel(replyTimeout)
        emit { it.onError("refresh_failed", detail) }
        refreshIn(RETRY_MS)
    }

    /** Takes the answer's links ([updateDelivery]); the engine keeps running and cached files keep playing. */
    private fun applyRefresh(json: String?) {
        if (destroyed) return
        if (json == null) {
            emit { it.onError("refresh_failed", "getDelivery gave no delivery") }
            refreshIn(RETRY_MS)
            return
        }
        val before = delivery.expiresAt
        if (!updateDelivery(json)) {
            refreshIn(RETRY_MS)
            return
        }
        // Links that do not last longer are asked for again, not in a tight loop.
        val after = delivery.expiresAt
        if (before != null && after != null && after <= before) refreshIn(RETRY_MS)
    }

    companion object {
        /**
         * The `formats` to ask the release delivery for (player.md, "Asking
         * for AV1"): `av1` when this phone decodes AV1 in hardware, else
         * empty, and the delivery keeps its stacked H.264 files. The software
         * decoder Android 10 and later ship does not count: on some phones
         * its frames reach the screen as stripes.
         */
        @JvmStatic
        val formats: String by lazy { if (av1Decoder == Av1Decoder.HARDWARE) "av1" else "" }

        /** How this phone decodes AV1, so an app can say why it plays the H.264 files. */
        @JvmStatic
        val av1Decoder: Av1Decoder by lazy { runCatching { av1Decoder(installedDecoders()) }.getOrDefault(Av1Decoder.NONE) }

        private fun installedDecoders(): List<DecoderInfo> =
            MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filterNot { it.isEncoder }.map { info ->
                // Before Android 10 a codec cannot say it runs in hardware.
                val hardware = Build.VERSION.SDK_INT >= 29 && info.isHardwareAccelerated && !info.isSoftwareOnly
                DecoderInfo(info.supportedTypes.toList(), hardware)
            }

        /** Refresh this long before the links expire, or halfway when they expire sooner than twice this. */
        private const val REFRESH_LEAD_MS = 5 * 60_000L
        /** Try again this long after a failed refresh. */
        private const val RETRY_MS = 30_000L
        /** A getDelivery that has not replied after this long has failed. */
        private const val REPLY_TIMEOUT_MS = 60_000L
    }
}

/** How a phone decodes AV1 (player.md, "Asking for AV1"). */
enum class Av1Decoder {
    /** A hardware decoder: the player asks for AV1. */
    HARDWARE,
    /** Only a software decoder: the player plays the stacked H.264 files. */
    SOFTWARE,
    /** No AV1 decoder: the player plays the stacked H.264 files. */
    NONE,
}

/** One video decoder: the types it decodes and whether it runs in hardware. */
internal class DecoderInfo(val types: List<String>, val hardware: Boolean)

internal fun av1Decoder(decoders: List<DecoderInfo>): Av1Decoder {
    val av1 = decoders.filter { decoder -> decoder.types.any { it.equals("video/av01", ignoreCase = true) } }
    return when {
        av1.any { it.hardware } -> Av1Decoder.HARDWARE
        av1.isNotEmpty() -> Av1Decoder.SOFTWARE
        else -> Av1Decoder.NONE
    }
}

/** A seed for the engine's extra-clip roll; the engine turns 0 into 1. */
internal fun randomSeed(): ULong = Random.nextLong().toULong()

/** Play time: whole milliseconds from the start, stopped while paused (rules.md 11.2). */
internal class PlayClock(private val elapsedMs: () -> Long) {
    private var base = 0L
    private var since = 0L

    var running = false
        private set

    fun now(): Long = if (running) base + (elapsedMs() - since) else base

    fun resume() {
        if (running) return
        since = elapsedMs()
        running = true
    }

    fun pause() {
        if (!running) return
        base = now()
        running = false
    }
}
