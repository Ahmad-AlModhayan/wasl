package com.almodhayan.drivesync.ui.devices

import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.EncodeHintType
import com.google.zxing.LuminanceSource
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.BitMatrix
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel

/** Pairing codes as QR codes, made and read on the phone with ZXing (no Play services). */
object Qr {
    /** The code's modules, with a 2-module quiet zone. */
    fun encode(text: String): BitMatrix = QRCodeWriter().encode(
        text, BarcodeFormat.QR_CODE, 0, 0,
        mapOf(EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.M, EncodeHintType.MARGIN to 2, EncodeHintType.CHARACTER_SET to "UTF-8"),
    )

    /** Reads a QR code from a camera frame's luminance plane; null when there is none. */
    fun decode(y: ByteArray, rowStride: Int, width: Int, height: Int): String? =
        decode(PlanarYUVLuminanceSource(y, rowStride, height, 0, 0, width, height, false))

    fun decode(source: LuminanceSource): String? {
        val reader = QRCodeReader()
        val hints = mapOf(DecodeHintType.TRY_HARDER to true, DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE))
        return try {
            reader.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
        } catch (e: NotFoundException) {
            null
        } catch (e: Exception) {
            // A checksum or format error on a blurry frame: the next frame tries again.
            null
        } finally {
            reader.reset()
        }
    }

    /** A pasted or scanned code without the spaces and line breaks people copy with it. */
    fun clean(code: String): String = code.trim().filterNot { it.isWhitespace() }
}

/** Isolates [s] as a left-to-right run inside text of either direction (IDs, codes, addresses). */
fun ltr(s: String): String = "⁦$s⁩"
