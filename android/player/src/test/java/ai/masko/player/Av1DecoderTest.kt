package ai.masko.player

import org.junit.Assert.assertEquals
import org.junit.Test

/** Which AV1 decoder a phone has decides `formats` (spec/player.md, "Asking for AV1"). */
class Av1DecoderTest {
    private val h264 = DecoderInfo(listOf("video/avc"), hardware = true)

    @Test
    fun aHardwareDecoderIsHardware() =
        assertEquals(Av1Decoder.HARDWARE, av1Decoder(listOf(h264, DecoderInfo(listOf("video/av01"), hardware = true))))

    @Test
    fun hardwareWinsOverTheSoftwareDecoder() = assertEquals(
        Av1Decoder.HARDWARE,
        av1Decoder(listOf(DecoderInfo(listOf("video/av01"), hardware = false), DecoderInfo(listOf("video/av01"), hardware = true))),
    )

    /** A Xiaomi Mi A3 on Android 11 has only `c2.android.av1.decoder`, whose frames reach the screen as stripes. */
    @Test
    fun onlyTheSoftwareDecoderIsSoftware() =
        assertEquals(Av1Decoder.SOFTWARE, av1Decoder(listOf(h264, DecoderInfo(listOf("video/av01"), hardware = false))))

    @Test
    fun noAv1DecoderIsNone() = assertEquals(Av1Decoder.NONE, av1Decoder(listOf(h264)))

    @Test
    fun theTypeMatchesInAnyCase() =
        assertEquals(Av1Decoder.HARDWARE, av1Decoder(listOf(DecoderInfo(listOf("VIDEO/AV01"), hardware = true))))
}
