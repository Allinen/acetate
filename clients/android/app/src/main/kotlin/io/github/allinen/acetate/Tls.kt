package io.github.allinen.acetate

import android.content.Context

/** Gives the Rust side what it needs to check HTTPS certificates the way Android does. */
object Tls {
    init {
        System.loadLibrary("acetate_ffi")
    }

    @JvmStatic
    external fun init(context: Context)
}
