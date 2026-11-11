package com.almodhayan.drivesync.devices

import com.almodhayan.drivesync.ui.devices.Qr
import com.almodhayan.drivesync.ui.devices.ltr
import com.google.zxing.RGBLuminanceSource
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Pairing codes survive the trip through a QR code, and pasted codes lose their whitespace. */
class QrTest {
    private fun render(text: String, scale: Int = 4): RGBLuminanceSource {
        val m = Qr.encode(text)
        val w = m.width * scale
        val h = m.height * scale
        val px = IntArray(w * h) { i -> if (m.get((i % w) / scale, (i / w) / scale)) 0xFF000000.toInt() else 0xFFFFFFFF.toInt() }
        return RGBLuminanceSource(w, h, px)
    }

    @Test fun `a code encodes and decodes`() {
        val code = "WASL1:AB3DEF7HJKLMNPQRSTUVWXYZ234567ABCDEFGHJKLMNPQRSTUVWXYZ234567"
        assertEquals(code, Qr.decode(render(code)))
    }

    @Test fun `a blank frame has no code`() {
        assertNull(Qr.decode(RGBLuminanceSource(64, 64, IntArray(64 * 64) { 0xFFFFFFFF.toInt() })))
    }

    @Test fun `pasted codes lose spaces and line breaks`() {
        assertEquals("ABCD-EFGH", Qr.clean("  ABCD-\nEFGH \t"))
    }

    @Test fun `ids are isolated left to right`() {
        assertEquals("⁦WASL-ABCD⁩", ltr("WASL-ABCD"))
    }
}
