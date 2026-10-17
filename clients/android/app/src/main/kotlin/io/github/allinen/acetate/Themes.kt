package io.github.allinen.acetate

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.acetate_ffi.CustomTheme
import uniffi.acetate_ffi.Palette as ThemePalette
import uniffi.acetate_ffi.themeFind
import uniffi.acetate_ffi.themeIsPreset
import uniffi.acetate_ffi.themeNew
import uniffi.acetate_ffi.themePresets
import uniffi.acetate_ffi.themesFromJson
import uniffi.acetate_ffi.themesToJson

/**
 * Which theme is in use and the ones the user made, kept on the phone. The palettes
 * themselves are the core's; [Palette] holds the one in use.
 */
object Themes {
    private var prefs: android.content.SharedPreferences? = null

    var name by mutableStateOf(themePresets().first())
        private set

    var custom by mutableStateOf<List<CustomTheme>>(emptyList())
        private set

    fun init(context: Context) {
        val p = context.getSharedPreferences("theme", Context.MODE_PRIVATE)
        prefs = p
        custom = themesFromJson(p.getString("custom", "[]") ?: "[]")
        name = p.getString("name", null) ?: themePresets().first()
        Palette.current = themeFind(name, custom)
    }

    private fun save() {
        prefs?.edit()?.putString("name", name)?.putString("custom", themesToJson(custom))?.apply()
    }

    fun choose(theme: String) {
        name = theme
        Palette.current = themeFind(theme, custom)
        save()
    }

    /** `base`, or `base 2`, `base 3`… if that's taken. */
    private fun freeName(base: String): String =
        generateSequence(1) { it + 1 }.map { if (it == 1) base else "$base $it" }
            .first { !themeIsPreset(it) && custom.none { t -> t.name == it } }

    /** A copy of the theme in use, to change; it becomes the one in use. */
    fun addFromCurrent() {
        val theme = themeNew(freeName("My theme"), Palette.current)
        custom = custom + theme
        choose(theme.name)
    }

    /** The colors of the theme in use (a theme of the user's) become [palette]; kept with [keep]. */
    fun edit(palette: ThemePalette) {
        custom = custom.map { if (it.name == name) themeNew(name, palette) else it }
        Palette.current = palette
    }

    /** Writes what [edit] changed (not on every step of a drag). */
    fun keep() = save()

    /** False: the name is empty or another theme's. */
    fun rename(to: String): Boolean {
        val new = to.trim()
        if (new.isEmpty() || (new != name && (themeIsPreset(new) || custom.any { it.name == new }))) return false
        custom = custom.map { if (it.name == name) CustomTheme(new, it.colors) else it }
        name = new
        save()
        return true
    }

    fun delete() {
        custom = custom.filter { it.name != name }
        choose(themePresets().first())
    }
}
