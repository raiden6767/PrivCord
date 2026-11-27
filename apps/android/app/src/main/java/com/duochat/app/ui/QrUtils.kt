package com.duochat.app.ui

import android.app.Activity
import android.graphics.Bitmap
import android.view.WindowManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel

/** Shared QR-code bitmap helper used by ContactDialog and ManualConnectDialog. */
internal fun qrBitmap(text: String, size: Int = 900): Bitmap? = try {
    val hints = mapOf(
        EncodeHintType.MARGIN to 4, // the standard quiet zone: a code with less is harder for a camera to find
        EncodeHintType.CHARACTER_SET to "UTF-8",
        EncodeHintType.ERROR_CORRECTION to ErrorCorrectionLevel.L,
    )
    val matrix = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, size, size, hints)
    val pixels = IntArray(size * size)
    for (y in 0 until size) {
        val row = y * size
        for (x in 0 until size) {
            pixels[row + x] = if (matrix[x, y]) {
                android.graphics.Color.BLACK
            } else {
                android.graphics.Color.WHITE
            }
        }
    }
    Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888).apply {
        setPixels(pixels, 0, size, 0, 0, size, size)
    }
} catch (_: Exception) {
    null
}



/**
 * The side a QR code is shown at: most of the screen's width. An invite is a dense code (about 90 squares across), and a
 * small one has squares too small for a camera to tell apart; never so big it leaves the screen.
 */
@Composable
internal fun qrSide(): Dp = (LocalConfiguration.current.screenWidthDp * 0.78f).dp.coerceIn(200.dp, 340.dp)

/**
 * Full screen brightness while a QR code is on screen, put back afterwards: a dim screen is the usual reason a code will
 * not scan.
 */
@Composable
internal fun BrightScreenWhileShown() {
    val window = (LocalContext.current as? Activity)?.window
    DisposableEffect(window) {
        val before = window?.attributes?.screenBrightness
        window?.let { w -> w.attributes = w.attributes.also { it.screenBrightness = 1f } }
        onDispose {
            window?.let { w ->
                w.attributes = w.attributes.also {
                    it.screenBrightness = before ?: WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
                }
            }
        }
    }
}
