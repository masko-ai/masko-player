package ai.masko.player

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.net.URLDecoder
import java.net.URLEncoder
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ExecutorService
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** How a canvas read from its URL reaches the device; tests replace it. Both calls block. */
internal interface CanvasNetwork {
    /** An HTTP answer: its status and its body as text. */
    class Answer(val status: Int, val body: String)

    /** A file written by [download]: the status, and the `Content-Length` (-1 when unknown). */
    class Download(val status: Int, val length: Long)

    /** GETs [url] with [headers]; throws when it cannot be asked (offline). */
    fun get(url: String, headers: Map<String, String>): Answer

    /** Writes [url]'s body into [into]; throws when it cannot be fetched. */
    fun download(url: String, into: File): Download
}

/** The real network: `HttpURLConnection`, 30 seconds to connect and to read. */
internal object HttpCanvasNetwork : CanvasNetwork {
    private fun open(url: String): HttpURLConnection =
        (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 30_000
            readTimeout = 30_000
            useCaches = false
            instanceFollowRedirects = true
        }

    override fun get(url: String, headers: Map<String, String>): CanvasNetwork.Answer {
        val connection = open(url)
        try {
            headers.forEach(connection::setRequestProperty)
            val status = connection.responseCode
            val stream = if (status >= 400) connection.errorStream else connection.inputStream
            val body = stream?.use { it.readBytes().toString(Charsets.UTF_8) } ?: ""
            return CanvasNetwork.Answer(status, body)
        } finally {
            connection.disconnect()
        }
    }

    override fun download(url: String, into: File): CanvasNetwork.Download {
        val connection = open(url)
        try {
            val status = connection.responseCode
            if (status !in 200..299) return CanvasNetwork.Download(status, -1)
            val length = connection.contentLengthLong
            connection.inputStream.use { input -> into.outputStream().use { input.copyTo(it) } }
            return CanvasNetwork.Download(status, length)
        } finally {
            connection.disconnect()
        }
    }
}

/** Runs blocking work off the main thread. Tests run it in place. */
internal interface Background {
    fun run(task: () -> Unit)

    /** Takes no more work; work already running finishes. */
    fun close()

    companion object {
        /** One thread of its own, for one player, ended after 30 idle seconds: a forgotten player keeps no thread. */
        fun thread(): Background = object : Background {
            private val executor: ExecutorService = ThreadPoolExecutor(
                0, 1, 30, TimeUnit.SECONDS, LinkedBlockingQueue(),
            ) { r -> Thread(r, "MaskoCanvas").apply { isDaemon = true } }

            override fun run(task: () -> Unit) {
                if (!executor.isShutdown) executor.execute(task)
            }

            override fun close() = executor.shutdown()
        }
    }
}

/**
 * A canvas's latest release, read from its URL and kept on the device
 * (spec/player.md, "A canvas from its URL"). Every file is downloaded once and
 * kept, named after its link's path: the path never changes, only the
 * signature does. `release.json` holds the last release whose files were all
 * kept (its `release_id` and its `delivery`, as the API gives them), so the
 * mascot plays offline at the next launch.
 *
 * [latest], [release] and [keep] block: call them off the main thread.
 */
internal class KeptRelease(
    url: String,
    /** Null when the canvas URL carries its key (`?key=masko_app_...`). */
    private val appKey: String?,
    /** Where every canvas of the app keeps its files; see [root]. */
    private val root: File,
    /** [MaskoPlayer.formats], asked for with the release. */
    formats: String,
    private val network: CanvasNetwork = HttpCanvasNetwork,
) {
    /** A failure to ask or to download, with what failed. */
    class Failure(message: String) : Exception(message)

    /** A release as the canvas URL gives it: its id, to ask it again for fresh links, and its delivery. */
    class Release(val id: String?, val delivery: String)

    /** The canvas URL, with `target=android` and the player's `formats` unless it names them. */
    val request: String = request(url, formats)

    /** This canvas's folder: a digest of its URL without the query. */
    private val folder = File(root, digest(withoutQuery(url), 16))

    /** This canvas's kept files. */
    val files = File(folder, "files")

    private val keptFile get() = File(folder, "release.json")

    // ------------------------------------------------------------------ the kept release

    /** The kept release, when every file it plays is here. */
    fun kept(): Release? {
        val release = readKept() ?: return null
        val links = try {
            links(release.delivery)
        } catch (_: Exception) {
            return null
        }
        return release.takeIf { links.all(::has) }
    }

    private fun readKept(): Release? = try {
        val saved = JSONObject(keptFile.readText())
        Release(saved.optString("release_id").ifEmpty { null }, saved.getJSONObject("delivery").toString())
    } catch (_: Exception) {
        null
    }

    /** The local file for [link], named after its path. */
    fun file(link: String): File = File(files, name(link))

    fun has(link: String): Boolean = file(link).isFile

    /** The kept file of [link] as a `file://` URI, or null while it is not kept. */
    fun local(link: String): String? {
        if (!link.startsWith("https://") && !link.startsWith("http://")) return null
        val file = file(link)
        return if (file.isFile) "file://" + file.absolutePath else null
    }

    /** Deletes kept files the kept release does not play. The player calls it before it plays anything. */
    fun removeUnused() {
        val release = readKept() ?: return
        val used = try {
            links(release.delivery).map(::name).toSet()
        } catch (_: Exception) {
            return
        }
        if (used.isEmpty()) return
        files.listFiles()?.forEach { if (it.name !in used) it.delete() }
    }

    // ------------------------------------------------------------------ the latest release

    /** Asks the canvas URL for its latest release. */
    fun latest(): Release = ask(request)

    /** Asks for release [id] again, for fresh links: `.../canvases/{canvasId}/releases/{id}/delivery`, same query. */
    fun release(id: String): Release = ask(releaseRequest(request, id) ?: request)

    private fun ask(url: String): Release {
        val headers = LinkedHashMap<String, String>()
        if (!appKey.isNullOrEmpty()) headers["Authorization"] = "Bearer $appKey"
        headers["Masko-Install-Id"] = installId()
        val answer = network.get(url, headers)
        val body = try {
            JSONObject(answer.body)
        } catch (_: Exception) {
            null
        }
        if (answer.status != 200) {
            val message = body?.optJSONObject("error")?.optString("message")?.ifEmpty { null }
            throw Failure("the canvas URL answered ${answer.status}" + (message?.let { ": $it" } ?: ""))
        }
        val data = body?.optJSONObject("data")
        val delivery = data?.optJSONObject("delivery") ?: throw Failure("the canvas URL gave no delivery")
        return Release(data.optString("release_id").ifEmpty { null }, delivery.toString())
    }

    /** Downloads every file of [release] that is not here yet, then keeps it as the release to play, also after the next launch. */
    fun keep(release: Release) {
        for (link in links(release.delivery).sorted()) {
            if (!has(link)) download(link)
        }
        val saved = JSONObject().put("release_id", release.id ?: JSONObject.NULL).put("delivery", JSONObject(release.delivery))
        write(keptFile, saved.toString())
    }

    /** Downloads into a file next to its place, then renames it: a file is complete or not there. */
    private fun download(link: String) {
        files.mkdirs()
        val part = File(files, UUID.randomUUID().toString() + ".part")
        try {
            val got = network.download(link, part)
            val size = part.length()
            if (got.status !in 200..299 || size <= 0 || (got.length >= 0 && got.length != size)) {
                throw Failure("${lastSegment(link)} arrived incomplete (${got.status})")
            }
            if (!part.renameTo(file(link))) throw Failure("cannot keep ${lastSegment(link)}")
        } finally {
            part.delete()
        }
    }

    /**
     * A random id this installed copy keeps, sent as `Masko-Install-Id` so
     * the mascot's owner sees how many copies are active.
     */
    private fun installId(): String {
        val file = File(root, "install-id")
        try {
            val saved = file.readText().trim()
            UUID.fromString(saved)
            return saved
        } catch (_: Exception) {
            // None yet, or not a UUID: a new one.
        }
        val id = UUID.randomUUID().toString()
        try {
            write(file, id)
        } catch (_: Exception) {
            // Sent anyway; the next ask makes another.
        }
        return id
    }

    companion object {
        /** `noBackupFilesDir/MaskoPlayer`: everything in it can be downloaded again. */
        fun root(context: Context): File = File(context.noBackupFilesDir, "MaskoPlayer")

        /** [url] with `target=android` and `formats`, unless its query names them; the rest of its query is kept as is. */
        fun request(url: String, formats: String): String {
            val base = url.substringBefore('#')
            val query = base.substringAfter('?', "")
            val names = query.split('&').filter { it.isNotEmpty() }.map { decode(it.substringBefore('=')) }
            val added = ArrayList<String>()
            if ("target" !in names) added += "target=android"
            if ("formats" !in names) added += "formats=" + encode(formats)
            if (added.isEmpty()) return base
            val joiner = when {
                '?' !in base -> "?"
                base.endsWith("?") || base.endsWith("&") -> ""
                else -> "&"
            }
            return base + joiner + added.joinToString("&")
        }

        /** The release's own URL: the canvas URL's `/delivery` path made `/releases/{id}/delivery`, same query; null when the path does not end with `/delivery`. */
        fun releaseRequest(request: String, id: String): String? {
            val path = request.substringBefore('?')
            if (!path.endsWith("/delivery")) return null
            val query = request.substring(path.length)
            return path.removeSuffix("/delivery") + "/releases/" + encode(id) + "/delivery" + query
        }

        /** The file name for a link: a digest of its path, with its extension. */
        fun name(link: String): String {
            val path = try {
                URI(link).path ?: link
            } catch (_: Exception) {
                fileKey(link)
            }
            val last = path.substringAfterLast('/')
            val suffix = last.substringAfterLast('.', "").ifEmpty { "bin" }
            return digest(path, 64) + "." + suffix
        }

        /** Every link of a delivery's graph: clips, stills and sounds. */
        fun links(delivery: String): Set<String> {
            val links = HashSet<String>()
            fun walk(value: Any?) {
                when (value) {
                    is String -> if (value.startsWith("https://") || value.startsWith("http://")) links += value
                    is JSONArray -> for (i in 0 until value.length()) walk(value.opt(i))
                    is JSONObject -> value.keys().forEach { walk(value.opt(it)) }
                }
            }
            walk(JSONObject(delivery).optJSONObject("runtimeGraph"))
            return links
        }

        private fun withoutQuery(url: String) = url.substringBefore('#').substringBefore('?')

        private fun lastSegment(link: String) = fileKey(link).substringAfterLast('/')

        private fun digest(text: String, length: Int): String =
            MessageDigest.getInstance("SHA-256").digest(text.toByteArray())
                .joinToString("") { "%02x".format(it) }.take(length)

        private fun encode(text: String) = URLEncoder.encode(text, "UTF-8").replace("+", "%20")

        private fun decode(text: String) = try {
            URLDecoder.decode(text, "UTF-8")
        } catch (_: Exception) {
            text
        }

        /** Writes next to the file, then renames it over the old one. */
        private fun write(file: File, text: String) {
            file.parentFile?.mkdirs()
            val part = File(file.parentFile, file.name + "." + UUID.randomUUID() + ".part")
            try {
                part.writeText(text)
                if (!part.renameTo(file)) throw Failure("cannot write ${file.name}")
            } finally {
                part.delete()
            }
        }
    }
}
