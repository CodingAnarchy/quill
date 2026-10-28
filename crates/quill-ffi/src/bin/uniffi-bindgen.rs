//! UniFFI's bindings generator, built from this workspace so its version always
//! matches the `uniffi` the library was compiled with. `ios/build-core.sh` runs
//! it as `cargo run -p quill-ffi --features bindgen --bin uniffi-bindgen`.

fn main() {
    uniffi::uniffi_bindgen_main();
}
