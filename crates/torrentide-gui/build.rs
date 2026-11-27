use std::env;
use std::path::PathBuf;

/// Each style is a compile-time choice, so the UI is compiled once per style into its own file.
const STYLES: [&str; 1] = ["fluent"];

fn main() {
    let ui_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("run by cargo")).join("ui");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("run by cargo"));
    for style in STYLES {
        let config = slint_build::CompilerConfiguration::new()
            .with_style(style.into())
            // Element queries in UI tests need it; ~22 KB, so it stays on rather than needing a test-only build.
            .with_debug_info(true);
        let dependencies =
            slint_build::compile_with_output_path(&ui_dir, out.join(format!("{style}.rs")), config)
                .expect("ui/slint-project.json compiles");
        // compile_with_output_path prints no cargo directives, unlike compile().
        for path in dependencies {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
