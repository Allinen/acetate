// Generates the Kotlin (and Swift) bindings from the built library:
// `uniffi-bindgen generate --library libacetate_ffi.so --language kotlin --out-dir …`.
fn main() {
    uniffi::uniffi_bindgen_main()
}
