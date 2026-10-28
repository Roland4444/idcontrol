//! Reproducible, independent UI identifiers embedded in the Rust binary.
//! Cargo reruns this script when relevant files change. Set IDCONTROL_UI_REVISION
//! to a release tag to force a new client revision even if the file bytes match.
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

const HTML_FILES: &[&str] = &[
    "src/web.rs", "src/portal.rs", "src/access.rs", "src/contractor.rs", "src/project_sources.rs",
    "assets/bitrix-client.html", "assets/employee-picker.html",
];
const JS_FILES: &[&str] = &[
    "src/web.rs", "src/portal.rs", "src/access.rs", "src/contractor.rs", "src/project_sources.rs",
    "assets/project-sources.js", "assets/bitrix-client.js", "assets/employee-picker.js",
    "assets/ui-version-watch.js", "assets/contractor-upload.js",
    "assets/contractor-link-copy.js", "assets/package-actions.js",
];
const CSS_FILES: &[&str] = &[
    "assets/portal.css", "assets/bitrix-client.html", "src/web.rs",
];
const IMAGE_FILES: &[&str] = &[
    "assets/projects/federation.webp", "assets/projects/okland.webp",
    "assets/projects/tetris.webp", "assets/projects/kosmo.webp",
    "assets/projects/prime-tower.webp", "assets/projects/duo.webp",
];

fn id(label: &str, files: &[&str], revision: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(label.as_bytes());
    hash.update(revision.as_bytes());
    for file in files {
        println!("cargo:rerun-if-changed={file}");
        hash.update(file.as_bytes());
        hash.update(fs::read(Path::new(file)).unwrap_or_else(|e| panic!("Cannot read {file}: {e}")));
    }
    let hex = format!("{:x}", hash.finalize());
    format!("{}-{}", label, &hex[..16])
}
fn main() {
    println!("cargo:rerun-if-env-changed=IDCONTROL_UI_REVISION");
    println!("cargo:rerun-if-changed=build.rs");
    let revision = env::var("IDCONTROL_UI_REVISION").unwrap_or_default();
    println!("cargo:rustc-env=IDCONTROL_UI_HTML_ID={}", id("html", HTML_FILES, &revision));
    println!("cargo:rustc-env=IDCONTROL_UI_JS_ID={}", id("js", JS_FILES, &revision));
    println!("cargo:rustc-env=IDCONTROL_UI_CSS_ID={}", id("css", CSS_FILES, &revision));
    println!("cargo:rustc-env=IDCONTROL_UI_IMAGES_ID={}", id("images", IMAGE_FILES, &revision));
}
