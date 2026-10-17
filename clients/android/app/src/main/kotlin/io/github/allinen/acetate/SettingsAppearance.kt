package io.github.allinen.acetate

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import uniffi.acetate_ffi.themeColors
import uniffi.acetate_ffi.themePresets
import uniffi.acetate_ffi.themeWithColor

/** Settings, Appearance: the color theme; a built-in one, or one made here. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AppearanceTab() {
    Text("Theme", style = MaterialTheme.typography.titleSmall)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        (themePresets() + Themes.custom.map { it.name }).forEach { theme ->
            FilterChip(theme == Themes.name, { Themes.choose(theme) }, { Text(theme) })
        }
    }
    Spacer(Modifier.height(20.dp))
    Text("Your own", style = MaterialTheme.typography.titleSmall)
    TextButton({ Themes.addFromCurrent() }) { Text("New theme from this one") }
    if (Themes.custom.none { it.name == Themes.name }) {
        Text(
            "Makes a copy of the theme in use that you can change the colors of.",
            style = MaterialTheme.typography.bodySmall, color = Palette.faint,
        )
        return
    }
    ThemeEditor()
}

/** The name and colors of the theme in use, which is one of the user's. */
@Composable
private fun ThemeEditor() {
    // The name being typed; it's taken when the field is left, or put back if it's empty or
    // another theme's.
    var typed by remember(Themes.name) { mutableStateOf(Themes.name) }
    var open by remember(Themes.name) { mutableStateOf<Int?>(null) }
    OutlinedTextField(
        typed, { typed = it },
        Modifier.fillMaxWidth().onFocusChanged { if (!it.isFocused && !Themes.rename(typed)) typed = Themes.name },
        label = { Text("Name") }, singleLine = true,
    )
    Spacer(Modifier.height(8.dp))
    themeColors(Palette.current).forEachIndexed { index, color ->
        val swatch = Color(0xFF000000.toInt() or color.rgb.toInt())
        Row(
            Modifier.fillMaxWidth().clickable { open = if (open == index) null else index }.padding(vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(28.dp).background(swatch, RoundedCornerShape(6.dp)).border(1.dp, Palette.line, RoundedCornerShape(6.dp)))
            Spacer(Modifier.width(12.dp))
            Text(color.label, Modifier.weight(1f))
            Text("#%06x".format(color.rgb.toInt()), style = MaterialTheme.typography.labelMedium, color = Palette.muted)
        }
        if (open == index) ColorSliders(index, color.rgb)
    }
    Spacer(Modifier.height(8.dp))
    TextButton({ Themes.delete() }) { Text("Delete this theme") }
}

/** Red, green and blue sliders for one color; the change shows as it's dragged. */
@Composable
private fun ColorSliders(index: Int, rgb: UInt) {
    Column(Modifier.padding(start = 40.dp)) {
        listOf("R" to 16, "G" to 8, "B" to 0).forEach { (name, shift) ->
            val value = ((rgb shr shift) and 0xffu).toInt()
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(name, Modifier.width(20.dp), style = MaterialTheme.typography.labelMedium)
                Slider(
                    value.toFloat(),
                    {
                        val changed = (rgb and (0xffu shl shift).inv()) or (it.toInt().toUInt() shl shift)
                        Themes.edit(themeWithColor(Palette.current, index.toUInt(), changed))
                    },
                    Modifier.weight(1f),
                    onValueChangeFinished = { Themes.keep() },
                    valueRange = 0f..255f,
                )
                Text("$value", Modifier.width(32.dp), style = MaterialTheme.typography.labelMedium)
            }
        }
    }
}
