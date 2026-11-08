fn main() {
    // Pin the std-widgets style so the look never depends on whether Qt is installed.
    // Debug info lets the UI tests (tests/ui_flow.rs) find controls by accessible label.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(true);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint compilation failed");
}
