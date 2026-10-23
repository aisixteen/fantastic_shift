//! Packaging config checks for the desktop app (`app/tauri.conf.json`).

use std::path::PathBuf;

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("app")
}

fn load_conf() -> serde_json::Value {
    let path = app_dir().join("tauri.conf.json");
    let text = std::fs::read_to_string(&path).expect("read app/tauri.conf.json");
    serde_json::from_str(&text).expect("tauri.conf.json parses as JSON")
}

#[test]
fn tauri_conf_parses_and_names_the_team_app() {
    let conf = load_conf();
    assert_eq!(conf["productName"], "Fantastic Team");
    assert_eq!(conf["identifier"], "com.aisixteen.team");
}

#[test]
fn bundle_targets_cover_three_desktop_platforms() {
    let conf = load_conf();
    let targets: Vec<String> = conf["bundle"]["targets"]
        .as_array()
        .expect("bundle.targets is an array")
        .iter()
        .map(|t| t.as_str().unwrap_or_default().to_string())
        .collect();
    let has_any = |names: &[&str]| names.iter().any(|n| targets.iter().any(|t| t == n));
    assert!(
        has_any(&["app", "dmg"]),
        "macOS target missing: {targets:?}"
    );
    assert!(
        has_any(&["msi", "nsis"]),
        "Windows target missing: {targets:?}"
    );
    assert!(
        has_any(&["deb", "appimage"]),
        "Linux target missing: {targets:?}"
    );
}

#[test]
fn every_listed_icon_exists() {
    let conf = load_conf();
    let icons = conf["bundle"]["icon"]
        .as_array()
        .expect("bundle.icon is an array");
    assert!(!icons.is_empty(), "no icons listed");
    for icon in icons {
        let rel = icon.as_str().expect("icon entry is a string");
        let path = app_dir().join(rel);
        assert!(path.is_file(), "listed icon missing: {}", path.display());
    }
}

#[test]
fn no_signing_identity_is_committed() {
    let text = std::fs::read_to_string(app_dir().join("tauri.conf.json")).expect("read conf");
    for key in [
        "signingIdentity",
        "certificateThumbprint",
        "providerShortName",
    ] {
        assert!(!text.contains(key), "signing key committed: {key}");
    }
    for marker in ["APPLE_", "password"] {
        assert!(
            !text.contains(marker),
            "secret-like marker committed: {marker}"
        );
    }
}

#[test]
fn the_window_loads_a_loopback_url() {
    let conf = load_conf();
    let windows = conf["app"]["windows"]
        .as_array()
        .expect("app.windows is an array");
    assert!(!windows.is_empty(), "no window declared");
    for window in windows {
        let url = window["url"].as_str().expect("window url is a string");
        let host_ok = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
        assert!(host_ok, "window url is not loopback: {url}");
    }
}
