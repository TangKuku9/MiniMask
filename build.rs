use std::{env, fs, path::Path};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dist = Path::new(&manifest_dir).join("web-ui").join("dist");

    // Re-run the build script when the frontend sources or dist change so that
    // rust-embed picks up newly built assets.
    println!("cargo:rerun-if-changed=web-ui/src");
    println!("cargo:rerun-if-changed=web-ui/index.html");
    println!("cargo:rerun-if-changed=web-ui/package.json");
    println!("cargo:rerun-if-changed=web-ui/dist");

    // If the frontend has not been built yet, emit a minimal placeholder so the
    // Rust crate still compiles. The real UI is produced by `npm run build`.
    if !dist.join("index.html").exists() {
        fs::create_dir_all(&dist).expect("create dist dir");
        fs::write(dist.join("index.html"), FALLBACK_HTML).expect("write fallback index");
        println!(
            "cargo:warning=MiniMask: web-ui/dist not found, created a placeholder. \
             Run `npm install && npm run build` inside ./web-ui for the real UI."
        );
    }
}

const FALLBACK_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>MiniMask</title><style>body{font-family:system-ui,sans-serif;max-width:42rem;margin:4rem auto;padding:0 1rem;color:#1f2937}code{background:#f3f4f6;padding:.15rem .4rem;border-radius:.25rem}</style></head><body><h2>MiniMask Web UI is not built</h2><p>Run <code>npm install &amp;&amp; npm run build</code> inside the <code>web-ui</code> directory, then rebuild the server.</p></body></html>"#;
