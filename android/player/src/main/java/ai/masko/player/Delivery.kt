package ai.masko.player

import org.json.JSONArray
import org.json.JSONObject
import java.util.Calendar
import java.util.TimeZone

/**
 * The parts of a release delivery the player reads itself: media links, sounds
 * and host data. The engine reads every rule from the same JSON.
 */
internal class Delivery(
    val autoPlay: Boolean,
    val edges: Map<String, EdgeMedia>,
    /** Each pose's `transparentThumbnailUrl`. */
    val stills: Map<String, String>,
    /** `deliveryAccess.signedUrlExpiresAt`, in milliseconds since 1970. */
    val expiresAt: Long?,
    /** The release with its links left out: `release.id` and the graph. */
    private val shape: String?,
    /** The graph's `initialNode`. */
    val initialNode: String? = null,
) {
    /** The pose's main clip: its first pose clip without conditions (rules.md 7.1). */
    fun mainClip(pose: String): String? =
        edges.values.firstOrNull { it.isLoop && it.source == pose && !it.conditional && it.hasFile }?.id

    /**
     * Whether [other] is a delivery of the same release, so its links can
     * replace these: the same `release.id` and the same graph, links left out.
     */
    fun sameRelease(other: Delivery): Boolean = shape != null && shape == other.shape

    companion object {
        /** No delivery yet: what a lone clip has until its file's length is read. */
        val NONE = Delivery(true, emptyMap(), emptyMap(), null, null)

        /**
         * The delivery a lone clip plays as (player.md, "One clip"): one pose,
         * `clip`, whose loop is the stacked file, [seconds] long, with its own
         * voice when the file has a sound track ([voiced]).
         */
        fun clip(stacked: String, seconds: Double, voiced: Boolean): String {
            val edge = JSONObject()
                .put("id", "clip").put("source", "clip").put("target", "clip")
                .put("isLoop", true).put("duration", seconds)
                .put("videos", JSONObject().put("stacked", stacked))
                .put("hasAudio", voiced)
            val graph = JSONObject()
                .put("runtimeGraphSchemaVersion", 1)
                .put("initialNode", "clip")
                .put("nodes", JSONArray().put(JSONObject().put("id", "clip")))
                .put("edges", JSONArray().put(edge))
                .put("inputs", JSONArray())
            return JSONObject()
                .put("deliverySchemaVersion", 1)
                .put("playerContract", 1)
                .put("requires", JSONArray())
                .put("runtimeGraph", graph)
                .toString()
        }

        /** Reads the delivery, or null when it is not a JSON object with a graph. */
        fun parse(json: String): Delivery? = try {
            read(JSONObject(json))
        } catch (_: Exception) {
            null
        }

        private fun read(root: JSONObject): Delivery? {
            val graph = root.optJSONObject("runtimeGraph") ?: return null
            val edges = LinkedHashMap<String, EdgeMedia>()
            graph.optJSONArray("edges")?.objects()?.forEach { e ->
                val id = e.optString("id").ifEmpty { return@forEach }
                edges[id] = EdgeMedia.read(id, e)
            }
            val stills = HashMap<String, String>()
            graph.optJSONArray("nodes")?.objects()?.forEach { n ->
                val url = n.optText("transparentThumbnailUrl") ?: return@forEach
                stills[n.optString("id")] = url
            }
            val access = root.optJSONObject("deliveryAccess")
            return Delivery(
                autoPlay = graph.optBoolean("autoPlay", true),
                edges = edges,
                stills = stills,
                expiresAt = access?.optText("signedUrlExpiresAt")?.let(::parseIsoTime),
                shape = "[" + withoutLinks(root.optJSONObject("release")?.opt("id")) + "," + withoutLinks(graph) + "]",
                initialNode = graph.optText("initialNode"),
            )
        }

        private val LINK = Regex("^(https?|blob|data|file):", RegexOption.IGNORE_CASE)

        /** A JSON value with sorted keys and every link blanked, as text. */
        private fun withoutLinks(value: Any?): String = when (value) {
            is JSONObject -> value.keys().asSequence().sorted()
                .joinToString(",", "{", "}") { JSONObject.quote(it) + ":" + withoutLinks(value.opt(it)) }
            is JSONArray -> (0 until value.length()).joinToString(",", "[", "]") { withoutLinks(value.opt(it)) }
            is String -> JSONObject.quote(if (LINK.containsMatchIn(value)) "" else value)
            null, JSONObject.NULL -> "null"
            else -> value.toString()
        }
    }
}

internal class EdgeMedia(
    val id: String,
    val source: String,
    val target: String,
    val isLoop: Boolean,
    val conditional: Boolean,
    /** The base stacked file, AV1 or H.264: colour above, transparency below. */
    val stacked: String?,
    /** Stacked size variants, smallest first (manifest mode). */
    val variants: List<Pair<Int, String>>,
    val sound: Sound?,
    val hasAudio: Boolean,
    /** Host data for the app (player.md: `movement`). */
    val movement: JSONObject?,
    /** The clip's length in seconds at rate 1. */
    val duration: Double,
) {
    val hasFile get() = stacked != null || variants.isNotEmpty()

    /**
     * The stacked file for a mascot [sizePx] pixels across on screen: the
     * smallest variant at least that large, else the base file (player.md,
     * "The file and the size").
     */
    fun video(sizePx: Int): String? {
        if (sizePx > 0) variants.firstOrNull { it.first >= sizePx }?.let { return it.second }
        return stacked ?: variants.lastOrNull()?.second
    }

    companion object {
        fun read(id: String, e: JSONObject): EdgeMedia {
            val videos = e.optJSONObject("videos")
            // AV1 and H.264 stacked files share one layout; a delivery asked
            // with `formats=av1` names `av1` (player.md, "The file and the size").
            val kind = if (videos != null && names(videos, "av1")) "av1" else "stacked"
            val variants = ArrayList<Pair<Int, String>>()
            videos?.optJSONObject("variants")?.let { all ->
                for (key in all.keys()) {
                    val size = key.toIntOrNull() ?: continue
                    val url = all.optJSONObject(key)?.optText(kind) ?: continue
                    variants += size to url
                }
            }
            variants.sortBy { it.first }
            val sound = e.optJSONObject("sound")?.let { s ->
                s.optText("url")?.let { url ->
                    Sound(url, s.optDouble("volume", 1.0).toFloat().coerceIn(0f, 1f), s.optBoolean("loop", false))
                }
            }
            return EdgeMedia(
                id = id,
                source = e.optString("source"),
                target = e.optString("target"),
                isLoop = e.optBoolean("isLoop", false),
                conditional = hasConditions(e),
                stacked = videos?.optText(kind),
                variants = variants,
                sound = sound,
                hasAudio = e.optBoolean("hasAudio", false),
                movement = e.optJSONObject("movement"),
                duration = e.optDouble("duration", 0.0),
            )
        }

        /** Whether [videos] names a [kind] file, at its base or in a size of `variants`. */
        private fun names(videos: JSONObject, kind: String): Boolean {
            if (videos.optText(kind) != null) return true
            val all = videos.optJSONObject("variants") ?: return false
            return all.keys().asSequence().any { all.optJSONObject(it)?.optText(kind) != null }
        }

        /** A non-empty `conditionGroups` replaces `conditions`; empty groups are dropped (rules.md 2.1). */
        private fun hasConditions(e: JSONObject): Boolean {
            val groups = e.optJSONArray("conditionGroups")
            if (groups != null && groups.length() > 0) {
                return (0 until groups.length()).any { (groups.optJSONArray(it)?.length() ?: 0) > 0 }
            }
            return (e.optJSONArray("conditions")?.length() ?: 0) > 0
        }
    }
}

internal class Sound(val url: String, val volume: Float, val loop: Boolean)

private fun JSONArray.objects(): List<JSONObject> = (0 until length()).mapNotNull { optJSONObject(it) }

/** A string field, or null when missing, null or empty. */
private fun JSONObject.optText(name: String): String? =
    if (isNull(name)) null else optString(name).ifEmpty { null }

private val ISO_TIME = Regex(
    """(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(Z|z|[+-]\d{2}(?::?\d{2})?)?"""
)

/** An ISO 8601 date and time, in milliseconds since 1970; UTC when it has no offset. */
internal fun parseIsoTime(text: String): Long? {
    val m = ISO_TIME.matchEntire(text.trim()) ?: return null
    val g = m.groupValues
    val calendar = Calendar.getInstance(TimeZone.getTimeZone("UTC")).apply {
        clear()
        set(g[1].toInt(), g[2].toInt() - 1, g[3].toInt(), g[4].toInt(), g[5].toInt(), g[6].toInt())
    }
    val millis = g[7].padEnd(3, '0').take(3).toLong()
    val zone = g[8]
    val offsetMs = if (zone.isEmpty() || zone == "Z" || zone == "z") 0L else {
        val digits = zone.drop(1).replace(":", "")
        val hours = digits.take(2).toLong()
        val minutes = digits.drop(2).ifEmpty { "0" }.toLong()
        (if (zone[0] == '-') -1 else 1) * (hours * 60 + minutes) * 60_000
    }
    return calendar.timeInMillis + millis - offsetMs
}

/** A media link without its query, so a refreshed signed link names the same file. */
internal fun fileKey(url: String): String = url.substringBefore('?').substringBefore('#')

/**
 * The longer side, in pixels, of a mascot fitted inside a [width] x [height]
 * view, for its size variant: the mascot's own [aspect] (width over height)
 * once a clip is known, else square. 0 before the view has a size.
 */
internal fun fittedSizePx(width: Int, height: Int, aspect: Float?): Int {
    if (width <= 0 || height <= 0) return 0
    val ratio = aspect?.takeIf { it > 0f } ?: 1f
    val fittedWidth = minOf(width.toFloat(), height * ratio)
    val fittedHeight = fittedWidth / ratio
    return Math.round(maxOf(fittedWidth, fittedHeight))
}
