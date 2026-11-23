package com.duochat.app.ui

import android.graphics.Color as AndroidColor
import androidx.compose.foundation.background
import androidx.compose.material.icons.filled.Lock
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import com.duochat.app.R
import com.duochat.app.data.AppSettings
import com.duochat.app.data.accentFill
import com.duochat.app.data.mixColors
import com.duochat.app.data.themeById

/*
 * The look ("plum & apricot"): a warm paper ground by day and a warm near-black by night, ink text, and colour with
 * jobs. Plum is yours: your messages and the main action on a screen. Apricot is "new": unread counts and the reactions
 * you gave. Everything else (chips, the New chat button, secondary buttons) is ink on paper, so
 * the two colours keep their meaning. Figtree for every word; surfaces step by tone, not by shadow.
 */

@OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
private fun figtree(weight: Int) = Font(
    R.font.figtree,
    weight = FontWeight(weight),
    variationSettings = FontVariation.Settings(FontVariation.weight(weight)),
)

private val Figtree = FontFamily(figtree(400), figtree(500), figtree(600), figtree(700), figtree(800))

private fun style(size: Int, line: Int, weight: Int, tracking: Double = 0.0) = TextStyle(
    fontFamily = Figtree,
    fontSize = size.sp,
    lineHeight = line.sp,
    fontWeight = FontWeight(weight),
    letterSpacing = tracking.em,
)

private val typography = Typography(
    displaySmall = style(34, 40, 800, -0.025),
    headlineLarge = style(30, 36, 800, -0.02),
    headlineMedium = style(26, 32, 700, -0.015),
    headlineSmall = style(22, 28, 700, -0.01),
    titleLarge = style(20, 26, 700, -0.005),
    titleMedium = style(16, 22, 600),
    titleSmall = style(14, 20, 600),
    bodyLarge = style(16, 23, 400),
    bodyMedium = style(14, 20, 400),
    bodySmall = style(12, 16, 400, 0.005),
    labelLarge = style(14, 20, 600, 0.005),
    labelMedium = style(12, 16, 600, 0.01),
    labelSmall = style(11, 14, 500, 0.02),
)

private val shapes = Shapes(
    extraSmall = RoundedCornerShape(8.dp),
    small = RoundedCornerShape(12.dp),
    medium = RoundedCornerShape(16.dp),
    large = RoundedCornerShape(24.dp),
    extraLarge = RoundedCornerShape(28.dp),
)

private fun mix(a: Color, b: Color, t: Float) = Color(mixColors(a.toArgb(), b.toArgb(), t))

/** [fg] moved towards [toward] until it reads at 4.5:1 on every colour in [against]. */
private fun ensureReadable(fg: Color, against: List<Color>, toward: Color): Color {
    var c = fg
    var i = 0
    while (i < 25 && against.any { com.duochat.app.data.contrastRatio(c.toArgb(), it.toArgb()) < 4.5 }) {
        c = mix(c, toward, 0.08f)
        i++
    }
    return c
}

/** White or black, whichever reads better on [bg] (always at least 4.5:1). */
private fun onColor(bg: Color): Color =
    if (com.duochat.app.data.contrastRatio(Color.White.toArgb(), bg.toArgb()) >= com.duochat.app.data.contrastRatio(Color.Black.toArgb(), bg.toArgb())) Color.White else Color.Black

/**
 * Every surface role from three colours (ground, card, text), so built-in palettes and the default share one recipe.
 * Every text colour is checked against every surface it can sit on (ThemesTest.every role pair reads at 4.5 to 1).
 */
internal fun scheme(dark: Boolean, bg: Color, card: Color, text: Color, accent: Color): ColorScheme {
    val fill = Color(accentFill(accent.toArgb()))
    val base = if (dark) darkColorScheme() else lightColorScheme()
    val step = { t: Float -> mix(bg, text, t) }
    val high = step(if (dark) 0.10f else 0.055f)
    val highest = step(if (dark) 0.14f else 0.085f)
    val surfaces = listOf(bg, card, high, highest)
    // Secondary text: as light as it can be while still reading at 4.5:1 on the ground and on every tonal surface.
    var t = 0.30f
    var muted = mix(text, bg, t)
    while (t > 0f && surfaces.any { com.duochat.app.data.contrastRatio(muted.toArgb(), it.toArgb()) < 4.5 }) {
        t = (t - 0.02f).coerceAtLeast(0f)
        muted = mix(text, bg, t)
    }
    // The accent as text and as a button fill: moved until it reads on every surface, with black or white on top.
    val primary = if (dark) ensureReadable(accent, surfaces, Color.White) else ensureReadable(fill, surfaces, Color.Black)
    val secondary = if (dark) mix(accent, Color.White, 0.35f) else fill
    val errorContainer = if (dark) Color(0xFF3B1715) else Color(0xFFFDE7E5)
    val error = ensureReadable(if (dark) Color(0xFFFF8A80) else Color(0xFFC62828), surfaces + errorContainer, if (dark) Color.White else Color.Black)
    return base.copy(
        primary = primary,
        onPrimary = onColor(primary),
        primaryContainer = mix(bg, accent, if (dark) 0.22f else 0.14f),
        onPrimaryContainer = if (dark) mix(accent, Color.White, 0.55f) else mix(accent, Color.Black, 0.45f),
        secondary = secondary,
        onSecondary = onColor(secondary),
        secondaryContainer = mix(bg, accent, if (dark) 0.16f else 0.10f),
        onSecondaryContainer = text,
        tertiary = Apricot,
        onTertiary = Color(0xFF2B1608),
        tertiaryContainer = if (dark) Color(0xFF3A2516) else Color(0xFFFCE6D6),
        onTertiaryContainer = if (dark) Color(0xFFFFD2B3) else Color(0xFF5A2A0C),
        background = bg,
        onBackground = text,
        surface = bg,
        onSurface = text,
        surfaceVariant = high,
        onSurfaceVariant = muted,
        surfaceTint = Color.Transparent,
        surfaceBright = card,
        surfaceDim = step(if (dark) 0.02f else 0.06f),
        surfaceContainerLowest = card,
        surfaceContainerLow = mix(bg, card, 0.5f),
        surfaceContainer = card,
        surfaceContainerHigh = high,
        surfaceContainerHighest = highest,
        inverseSurface = text,
        inverseOnSurface = bg,
        outline = step(if (dark) 0.22f else 0.18f),
        outlineVariant = step(if (dark) 0.12f else 0.09f),
        error = error,
        errorContainer = errorContainer,
        onErrorContainer = if (dark) Color(0xFFFFD7D2) else Color(0xFF5F1410),
    )
}

@Composable
fun DuoChatTheme(settings: AppSettings, content: @Composable () -> Unit) {
    val themed = themeById(settings.palette)
    val dark = themed?.dark ?: when (settings.theme) {
        "dark" -> true
        "light" -> false
        else -> isSystemInDarkTheme()
    }
    val colors = if (themed != null) {
        scheme(themed.dark, Color(themed.bg), Color(themed.surface), Color(themed.text), Color(themed.accent))
    } else if (dark) {
        scheme(true, Color(0xFF131112), Color(0xFF1C191B), Color(0xFFF1ECEA), parseColor(settings.accent))
    } else {
        scheme(false, Color(0xFFF7F3EE), Color(0xFFFFFFFF), Color(0xFF221C1F), parseColor(settings.accent))
    }
    val accent = themed?.let { Color(it.accent) } ?: parseColor(settings.accent)
    androidx.compose.runtime.CompositionLocalProvider(LocalOwnFill provides Color(accentFill(accent.toArgb()))) {
        MaterialTheme(colorScheme = colors, typography = typography, shapes = shapes, content = content)
    }
}

/** The deep accent behind white text: your own message bubbles and the brand mark (same colour in light and dark). */
val LocalOwnFill = androidx.compose.runtime.staticCompositionLocalOf { Color(0xFF7A3E73) }

/** Positive state (connected, online): one green that reads on both grounds. */
val Online = Color(0xFF2E9E6B)

/** "New": unread counts and your reactions. Ink text on it. */
val Apricot = Color(0xFFF2A65A)

private fun parseColor(value: String): Color = try {
    Color(AndroidColor.parseColor(value))
} catch (_: Exception) {
    Color(0xFF7A3E73)
}

/** The app's icon, wherever the app shows its mark (onboarding, lock and loading screens). */
@Composable
fun BrandMark(size: Int = 56) {
    androidx.compose.foundation.Image(
        painter = androidx.compose.ui.res.painterResource(R.drawable.app_icon),
        contentDescription = null,
        modifier = androidx.compose.ui.Modifier.size(size.dp),
    )
}

/**
 * The one text field style: a tonal fill, no outline, rounded; the label floats inside. The Android twin of the
 * desktop's fields. Takes the parameters the screens use (positional value and onValueChange first, like the M3 one).
 */
@Composable
fun DuoTextField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: androidx.compose.ui.Modifier = androidx.compose.ui.Modifier,
    label: (@Composable () -> Unit)? = null,
    placeholder: (@Composable () -> Unit)? = null,
    singleLine: Boolean = false,
    minLines: Int = 1,
    maxLines: Int = if (singleLine) 1 else Int.MAX_VALUE,
    readOnly: Boolean = false,
    textStyle: TextStyle = MaterialTheme.typography.bodyLarge,
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
    keyboardActions: androidx.compose.foundation.text.KeyboardActions = androidx.compose.foundation.text.KeyboardActions.Default,
    visualTransformation: androidx.compose.ui.text.input.VisualTransformation = androidx.compose.ui.text.input.VisualTransformation.None,
) {
    val fill = MaterialTheme.colorScheme.surfaceContainerHigh
    androidx.compose.material3.TextField(
        value = value, onValueChange = onValueChange, modifier = modifier, label = label, placeholder = placeholder,
        singleLine = singleLine, minLines = minLines, maxLines = maxLines, readOnly = readOnly, textStyle = textStyle,
        keyboardOptions = keyboardOptions, keyboardActions = keyboardActions, visualTransformation = visualTransformation,
        shape = RoundedCornerShape(16.dp),
        colors = androidx.compose.material3.TextFieldDefaults.colors(
            focusedContainerColor = fill,
            unfocusedContainerColor = fill,
            disabledContainerColor = fill,
            focusedIndicatorColor = Color.Transparent,
            unfocusedIndicatorColor = Color.Transparent,
            disabledIndicatorColor = Color.Transparent,
            focusedLabelColor = MaterialTheme.colorScheme.onSurface,
            cursorColor = MaterialTheme.colorScheme.onSurface,
        ),
    )
}

/**
 * Items side by side (buttons, chips), wrapping onto the next line when the screen or the font size leaves no room,
 * instead of squeezing them or running off the edge. Used for every row of buttons or chips.
 */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun WrapRow(gap: androidx.compose.ui.unit.Dp, content: @Composable () -> Unit) {
    androidx.compose.foundation.layout.FlowRow(
        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(gap),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(gap),
    ) { content() }
}
