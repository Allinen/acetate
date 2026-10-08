package io.github.allinen.acetate

import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.material3.Typography
import androidx.compose.ui.Modifier
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight

/** The desktop client's palette (clients/desktop/src/ui/theme.rs): warm amber on near-black. */
object Palette {
    val bg = Color(0xFF0D0E12)
    val sidebar = Color(0xFF111217)
    val surface = Color(0xFF171920)
    val surfaceHi = Color(0xFF20232C)
    val line = Color(0xFF242731)
    val text = Color(0xFFECEDF1)
    val muted = Color(0xFF8E93A3)
    val faint = Color(0xFF555A68)
    val accent = Color(0xFFF5A65B)
    val accentSoft = Color(0xFF3A2B1F)
    val danger = Color(0xFFFF7878)
}

private val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_medium, FontWeight.Medium),
    Font(R.font.inter_semibold, FontWeight.SemiBold),
    Font(R.font.inter_bold, FontWeight.Bold),
)

private val colors = darkColorScheme(
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

/** Dark only, like the desktop client. */
@Composable
fun AcetateTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = colors, typography = Typography().inter()) {
        // The root surface sets the text color and background everything else inherits.
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background, content = content)
    }
}
