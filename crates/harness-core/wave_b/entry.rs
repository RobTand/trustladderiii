// Bazel-only wave B entry point. Cargo and the Gate do not discover these stubs.
#[test]
fn port_is_not_implemented() {
    panic!(
        "wave B entry point: port {} and replace this stub with its vector consumer",
        env!("WAVE_B_KIND")
    );
}
