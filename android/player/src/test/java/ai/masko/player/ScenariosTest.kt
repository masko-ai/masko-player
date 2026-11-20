package ai.masko.player

import ai.masko.player.engine.EngineCommand
import ai.masko.player.engine.EngineValue
import ai.masko.player.engine.LoadFailure
import ai.masko.player.engine.MaskoEngine
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized
import java.io.File

/**
 * Every scenario in spec/fixtures through the Kotlin binding, around the ideal
 * player of spec/fixtures/README.md: every clip loads and shows at once,
 * reported with `clipShown`, unless a step makes it fail or `loadMs` delays
 * it, and plays for its file's length divided by its rate (whole
 * milliseconds, halves to even). A port of engine/tests/scenarios.rs; both
 * must give the same trace.
 */
@RunWith(Parameterized::class)
class ScenariosTest(private val scenario: String) {

    @Test
    fun agreesWithTheRulebook() {
        val failures = try {
            run(File(FIXTURES, scenario))
        } catch (e: Throwable) {
            listOf("panicked: $e")
        }
        assertTrue("$scenario\n" + failures.joinToString("\n"), failures.isEmpty())
    }

    companion object {
        private val FIXTURES = File(System.getProperty("masko.fixtures") ?: "../../spec/fixtures")

        @JvmStatic
        @Parameterized.Parameters(name = "{0}")
        fun scenarios(): List<String> =
            FIXTURES.listFiles { f -> f.extension == "json" }!!.map { it.name }.sorted()
    }
}

private fun merge(base: JSONObject, patch: JSONObject) {
    for (k in patch.keys()) {
        val v = patch.get(k)
        when {
            v == JSONObject.NULL -> base.remove(k)
            v is JSONObject && base.optJSONObject(k) != null -> merge(base.getJSONObject(k), v)
            else -> base.put(k, v)
        }
    }
}

private fun buildDelivery(fx: JSONObject): JSONObject {
    val delivery = when (val canvas = fx.get("canvas")) {
        is String -> JSONObject(File(File(System.getProperty("masko.fixtures") ?: "../../spec/fixtures"), "canvases/$canvas.json").readText())
        else -> JSONObject(canvas.toString())
    }
    val patch = fx.optJSONObject("patch") ?: JSONObject()
    patch.optJSONObject("delivery")?.let { merge(delivery, it) }
    val g = delivery.getJSONObject("runtimeGraph")
    patch.optJSONObject("edges")?.let { edges ->
        for (id in edges.keys()) {
            val list = g.getJSONArray("edges")
            val idx = (0 until list.length()).first { list.getJSONObject(it).getString("id") == id }
            val change = edges.get(id)
            if (change == JSONObject.NULL) list.remove(idx) else merge(list.getJSONObject(idx), change as JSONObject)
        }
    }
    patch.optJSONArray("addEdges")?.let { add ->
        val list = g.getJSONArray("edges")
        for (i in 0 until add.length()) list.put(add.get(i))
    }
    if (!g.has("inputs")) g.put("inputs", JSONArray())
    patch.optJSONArray("addInputs")?.let { add ->
        val list = g.getJSONArray("inputs")
        for (i in 0 until add.length()) list.put(add.get(i))
    }
    patch.optJSONObject("defaults")?.let { defaults ->
        val inputs = g.getJSONArray("inputs")
        for (name in defaults.keys()) {
            val input = (0 until inputs.length()).map { inputs.getJSONObject(it) }.first { it.getString("name") == name }
            input.put("default", defaults.get(name))
        }
    }
    return delivery
}

private fun roundMs(x: Double): Long = Math.rint(x).coerceAtLeast(0.0).toLong()

/** The ideal player around the engine. */
private class IdealPlayer(
    val engine: MaskoEngine,
    /** Each edge's file length in seconds, when it differs from its duration. */
    val files: Map<String, Double>,
    val durations: Map<String, Double>,
    /** How long some edges' first frames take to show, in milliseconds. */
    val load: Map<String, Long>,
) {
    val noEnd = HashSet<String>()

    /** The clip the player has on screen: edge, shown at, rate. */
    var clip: Triple<String, Long, Double>? = null

    /** A clip asked for and not yet reported shown: edge, when it shows. */
    var showing: Pair<String, Long>? = null
    val events = ArrayList<String>()
    val preloaded = ArrayList<String>()

    fun now(): Long = engine.state().clock.toLong()

    fun absorb(commands: List<EngineCommand>) {
        val now = now()
        for (c in commands) {
            when (c) {
                is EngineCommand.ShowClip -> {
                    events += "shown:${c.edge}"
                    val at = now + (load[c.edge] ?: 0L)
                    clip = Triple(c.edge, at, c.rate)
                    showing = c.edge to at
                }
                is EngineCommand.ShowStill -> {
                    events += "shown:still:${c.pose}"
                    clip = null
                    showing = null
                }
                is EngineCommand.Preload -> preloaded += c.edge
                is EngineCommand.PoseEntered -> events += "pose:${c.pose}"
                is EngineCommand.Interrupted -> events += "interrupted:${c.edge}"
                is EngineCommand.Spent -> events += if (c.used) "spent:${c.name}" else "spent:${c.name}:unused"
                is EngineCommand.Error -> events += "error:${c.code}"
                is EngineCommand.PlaySound, is EngineCommand.StopSounds -> Unit
            }
        }
        // A clip that shows at once is reported shown at once.
        showing?.let { (edge, at) ->
            if (at == now) {
                showing = null
                absorb(engine.clipShown(edge, now.toULong()))
            }
        }
    }

    /** When the player reports the end of its clip, unless a step took the end signal away. */
    fun clipEnd(): Pair<String, Long>? {
        val (edge, shown, rate) = clip ?: return null
        if (edge in noEnd) return null
        val len = files[edge] ?: durations.getValue(edge)
        return edge to shown + roundMs(len / rate * 1000.0)
    }

    fun advanceTo(until: Long) {
        repeat(100_000) {
            showing?.let { (edge, at) ->
                val wake = engine.nextWake()?.toLong()
                if (at <= until && (wake == null || at <= wake)) {
                    showing = null
                    absorb(engine.clipShown(edge, at.toULong()))
                    return@repeat
                }
            }
            val end = clipEnd()
            val wake = engine.nextWake()?.toLong()
            val next = when {
                end != null && wake != null -> minOf(end.second, wake)
                end != null -> end.second
                else -> wake
            }
            if (next != null && next <= until) {
                val commands = if (end != null && end.second <= (wake ?: Long.MAX_VALUE)) {
                    check(engine.state().clip == end.first) {
                        "the player and the engine disagree on the clip on screen"
                    }
                    engine.clipEnded(end.first, end.second.toULong())
                } else {
                    engine.advance(next.toULong())
                }
                absorb(commands)
            } else {
                absorb(engine.advance(until.toULong()))
                return
            }
        }
        error("the clock did not settle")
    }

    fun runToEnd(moving: Boolean) {
        val state = engine.state()
        check(state.moving == moving) { "loopEnded or clipEnded on the wrong kind of clip" }
        val shownAt = state.shownAt.toLong()
        val expected = state.expectedMs.toLong()
        val safety = shownAt + expected + 1000
        val until = if (state.clip?.startsWith("still:") == true) {
            shownAt + expected
        } else {
            clipEnd()?.let { minOf(it.second, safety) } ?: safety
        }
        advanceTo(until)
    }
}

private fun act(p: IdealPlayer, step: JSONObject) {
    val e = p.engine
    val now = e.state().clock
    // `"instant": true` on a set or fire step (rules.md 5.8).
    val instant = step.optBoolean("instant", false)
    val commands: List<EngineCommand> = when {
        step.has("start") -> e.start(now)
        step.has("set") -> {
            val s = step.getJSONArray("set")
            when (val v = s.get(1)) {
                is Boolean -> e.setBool(s.getString(0), v, instant, now)
                is Number -> e.setNumber(s.getString(0), v.toDouble(), instant, now)
                else -> error("not a value: $v")
            }
        }
        step.has("fire") -> e.fire(step.getString("fire"), instant, now)
        step.has("advance") -> {
            p.advanceTo(now.toLong() + step.getLong("advance"))
            emptyList()
        }
        step.has("loopEnded") -> {
            p.runToEnd(false)
            emptyList()
        }
        step.has("clipEnded") -> {
            p.runToEnd(true)
            emptyList()
        }
        step.has("failClip") -> {
            e.markUnplayable(step.getString("failClip"))
            emptyList()
        }
        step.has("loadsAgain") -> {
            e.markPlayable(step.getString("loadsAgain"))
            emptyList()
        }
        step.has("noEndSignal") -> {
            p.noEnd += step.getString("noEndSignal")
            emptyList()
        }
        step.has("reset") -> e.reset(now)
        step.has("showPose") -> e.previewShowPose(step.getString("showPose"), now)
        step.has("playMove") -> e.previewPlayMove(step.getString("playMove"), now)
        else -> error("unknown step $step")
    }
    p.absorb(commands)
}

/** JSON as plain Kotlin values: maps, lists, strings, numbers, booleans and null. */
private fun plain(v: Any?): Any? = when (v) {
    null, JSONObject.NULL -> null
    is JSONObject -> v.keys().asSequence().associateWith { plain(v.get(it)) }
    is JSONArray -> (0 until v.length()).map { plain(v.get(it)) }
    else -> v
}

private fun observed(p: IdealPlayer, want: JSONObject): Map<String, Any?> {
    val s = p.engine.state()
    val out = HashMap<String, Any?>()
    out["clock"] = s.clock.toLong()
    out["pose"] = s.pose
    out["clip"] = s.clip
    out["next"] = s.nextClip?.let { clip ->
        mapOf("clip" to clip, "at" to (if (s.nextAtEnd) "end" else s.nextAt!!.toLong()))
    }
    out["events"] = p.events.toList()
    want.optJSONObject("values")?.let { names ->
        out["values"] = names.keys().asSequence().associateWith { name ->
            when (val v = p.engine.value(name)) {
                is EngineValue.Bool -> v.value
                is EngineValue.Number -> v.value
            }
        }
    }
    if (want.has("fired")) out["fired"] = p.engine.pendingFires()
    if (want.has("preloaded")) out["preloaded"] = p.preloaded.toList()
    return out
}

/** Equality as the scenarios mean it: a boolean equals the number 1 or 0. */
private fun same(a: Any?, b: Any?): Boolean {
    fun num(v: Any?): Double? = when (v) {
        is Boolean -> if (v) 1.0 else 0.0
        is Number -> v.toDouble()
        else -> null
    }
    if (a is List<*> && b is List<*>) return a.size == b.size && a.indices.all { same(a[it], b[it]) }
    if (a is Map<*, *> && b is Map<*, *>) {
        return a.size == b.size && a.all { (k, v) -> b.containsKey(k) && same(v, b[k]) }
    }
    val x = num(a)
    val y = num(b)
    return if (x != null && y != null) x == y else a == b
}

private fun run(path: File): List<String> {
    val fx = JSONObject(path.readText())
    val delivery = buildDelivery(fx)
    val wantRefusal = fx.optJSONObject("expectLoad")?.optString("refused")?.ifEmpty { null }
    val engine = try {
        MaskoEngine(delivery.toString())
    } catch (e: LoadFailure.Refused) {
        return if (wantRefusal == e.code) emptyList() else listOf("load refused with ${e.code}, expected $wantRefusal")
    }
    engine.use {
        if (wantRefusal != null) return listOf("loaded, expected refusal $wantRefusal")
        val rolls = fx.optJSONArray("random")?.let { a -> (0 until a.length()).map { a.getDouble(it) } } ?: emptyList()
        engine.useRolls(rolls)
        val files = fx.optJSONObject("files")?.let { o -> o.keys().asSequence().associateWith { o.getDouble(it) } } ?: emptyMap()
        val load = fx.optJSONObject("loadMs")?.let { o -> o.keys().asSequence().associateWith { o.getLong(it) } } ?: emptyMap()
        val edges = delivery.getJSONObject("runtimeGraph").getJSONArray("edges")
        val durations = (0 until edges.length()).map { edges.getJSONObject(it) }
            .associate { it.getString("id") to it.getDouble("duration") }
        val player = IdealPlayer(engine, files, durations, load)
        val failures = ArrayList<String>()
        val steps = fx.getJSONArray("steps")
        for (i in 0 until steps.length()) {
            val step = steps.getJSONObject(i)
            player.events.clear()
            player.preloaded.clear()
            act(player, step)
            val want = step.optJSONObject("expect") ?: JSONObject()
            val got = observed(player, want)
            for (k in want.keys()) {
                val g = got[k]
                val w = plain(want.get(k))
                if (!same(g, w)) failures += "step ${i + 1} $k: got $g, expected $w"
            }
        }
        return failures
    }
}
