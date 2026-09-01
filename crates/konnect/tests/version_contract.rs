use std::fs;
use std::path::{Path, PathBuf};

const WORKSPACE_PACKAGES: &[&str] = &[
    "konnect",
    "konnect-core",
    "konnect-ipc",
    "konnect-render",
    "konnect-schematic-editor",
    "konnect-sexp",
    "konnect-vcs",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("konnect crate is two levels below the repository root")
        .to_path_buf()
}

fn read_toml(path: &Path) -> toml::Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    toml::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn numeric_triplet(value: &str) -> [u64; 3] {
    let parts: Vec<_> = value.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "product and upstream versions must be numeric major.minor.patch"
    );
    std::array::from_fn(|index| {
        parts[index]
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("version component is not numeric: {value}"))
    })
}

fn assert_lock_versions(lock: &toml::Value, names: &[&str], expected: &str) {
    let packages = lock["package"]
        .as_array()
        .expect("Cargo.lock package array");
    for name in names {
        let package = packages
            .iter()
            .find(|package| package["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} is missing from Cargo.lock"));
        assert_eq!(
            package["version"].as_str(),
            Some(expected),
            "{name} lockfile version drifted"
        );
    }
}

#[test]
fn product_version_matches_provenance_and_artifacts() {
    let root = repo_root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let product_version = workspace["workspace"]["package"]["version"]
        .as_str()
        .expect("workspace package version");
    let provenance = &workspace["workspace"]["metadata"]["konnect-version"];
    let upstream_version = provenance["upstream_version"]
        .as_str()
        .expect("upstream_version metadata");
    let upstream_commit = provenance["upstream_commit"]
        .as_str()
        .expect("upstream_commit metadata");
    let fork_revision = provenance["fork_revision"]
        .as_integer()
        .expect("fork_revision metadata") as u64;

    assert!(
        upstream_commit.len() == 40
            && upstream_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "upstream_commit must be a full lowercase Git object ID"
    );
    assert!(
        (1..=99).contains(&fork_revision),
        "fork_revision must be between 1 and 99"
    );

    let upstream = numeric_triplet(upstream_version);
    let product = numeric_triplet(product_version);
    assert!(upstream[2] <= 154, "upstream patch exceeds the fork band");
    assert_eq!(product[0], upstream[0], "major version drifted");
    assert_eq!(product[1], upstream[1], "minor version drifted");
    assert_eq!(
        product[2],
        50_000 + upstream[2] * 100 + fork_revision,
        "product version does not match the documented fork formula"
    );
    assert!(
        product
            .iter()
            .all(|component| *component <= u16::MAX.into()),
        "product version component exceeds the Windows FILEVERSION limit"
    );

    let viewer = read_toml(&root.join("crates/schematic-viewer/Cargo.toml"));
    assert_eq!(
        viewer["package"]["version"].as_str(),
        Some(product_version),
        "viewer Cargo version drifted"
    );
    let tauri: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("crates/schematic-viewer/tauri.conf.json"))
            .expect("read viewer Tauri config"),
    )
    .expect("parse viewer Tauri config");
    assert_eq!(
        tauri["version"].as_str(),
        Some(product_version),
        "viewer Tauri version drifted"
    );

    assert_lock_versions(
        &read_toml(&root.join("Cargo.lock")),
        WORKSPACE_PACKAGES,
        product_version,
    );
    assert_lock_versions(
        &read_toml(&root.join("crates/schematic-viewer/Cargo.lock")),
        &[
            "schematic-viewer",
            "konnect-schematic-editor",
            "konnect-sexp",
        ],
        product_version,
    );
}
