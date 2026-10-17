package io.github.allinen.acetate

import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.material3.Typography
import androidx.compose.ui.Modifier
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.acetate_ffi.themeFind
import uniffi.acetate_ffi.themePresets
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight

/**
 * The colors in use, from the theme chosen in Settings (the palettes are the core's, the same
 * as the desktop's). Each is read from state, so the screens redraw when the theme changes.
 */
object Palette {
    var current by mutableStateOf(themeFind(themePresets().first(), emptyList()))

    private fun color(rgb: UInt) = Color(0xFF000000.toInt() or rgb.toInt())

    val bg get() = color(current.bg)
    val sidebar get() = color(current.sidebar)
    val surface get() = color(current.surface)
    val surfaceHi get() = color(current.surfaceHi)
    val line get() = color(current.line)
    val text get() = color(current.text)
    val muted get() = color(current.muted)
    val faint get() = color(current.faint)
    val accent get() = color(current.accent)
    val accentSoft get() = color(current.accentSoft)
    val danger get() = color(current.danger)
}

private val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_medium, FontWeight.Medium),
    Font(R.font.inter_semibold, FontWeight.SemiBold),
    Font(R.font.inter_bold, FontWeight.Bold),
)

/** The scheme for the palette in use: a light palette starts from Material's light colors. */
private fun colorScheme(): ColorScheme = (if (Palette.current.light) lightColorScheme() else darkColorScheme()).copy(
    primary = Palette.accent,
    onPrimary = Palette.bg,
    primaryContainer = Palette.accentSoft,
    onPrimaryContainer = Palette.accent,
    secondary = Palette.accent,
    onSecondary = Palette.bg,
    secondaryContainer = Palette.accentSoft,
    onSecondaryContainer = Palette.accent,
    tertiary = Palette.accent,
    background = Palette.bg,
    onBackground = Palette.text,
    surface = Palette.bg,
    onSurface = Palette.text,
    surfaceVariant = Palette.surfaceHi,
    onSurfaceVariant = Palette.muted,
    surfaceTint = Palette.accent,
    surfaceBright = Palette.surfaceHi,
    surfaceDim = Palette.bg,
    surfaceContainerLowest = Palette.bg,
    surfaceContainerLow = Palette.sidebar,
    surfaceContainer = Palette.surface,
    surfaceContainerHigh = Palette.surface,
    surfaceContainerHighest = Palette.surfaceHi,
    outline = Palette.faint,
    outlineVariant = Palette.line,
    error = Palette.danger,
    onError = Palette.bg,
    inverseSurface = Palette.text,
    inverseOnSurface = Palette.bg,
    inversePrimary = Palette.accent,
)

private fun Typography.inter(): Typography {
    fun TextStyle.f() = copy(fontFamily = Inter)
    return copy(
        displayLarge = displayLarge.f(), displayMedium = displayMedium.f(), displaySmall = displaySmall.f(),
        headlineLarge = headlineLarge.f(), headlineMedium = headlineMedium.f(), headlineSmall = headlineSmall.f(),
        titleLarge = titleLarge.f(), titleMedium = titleMedium.f(), titleSmall = titleSmall.f(),
        bodyLarge = bodyLarge.f(), bodyMedium = bodyMedium.f(), bodySmall = bodySmall.f(),
        labelLarge = labelLarge.f(), labelMedium = labelMedium.f(), labelSmall = labelSmall.f(),
    )
}

/** The colors are the theme chosen in Settings (see [Palette]). */
@Composable
fun AcetateTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = colorScheme(), typography = Typography().inter()) {
        // The root surface sets the text color and background everything else inherits.
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background, content = content)
    }
}
