package io.github.allinen.acetate

import androidx.compose.runtime.Composable

@Composable
fun Root(model: Model) {
    if (model.session == null) LoginScreen(model) else LibraryScreen(model)
}
