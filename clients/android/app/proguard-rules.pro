# The platform verifier is only called from Rust (JNI), which R8 can't see.
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }

# Rust calls this by its JNI name (Java_io_github_allinen_acetate_Tls_init).
-keep class io.github.allinen.acetate.Tls { *; }

# The Rust core's Kotlin side (uniffi) talks to the library through JNA, by reflection.
-keep class uniffi.acetate_ffi.** { *; }
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-dontwarn java.awt.**
