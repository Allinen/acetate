# The platform verifier is only called from Rust (JNI), which R8 can't see.
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }
