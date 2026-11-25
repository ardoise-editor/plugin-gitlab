//! The manifest as the editor reads it: each panel of the activity bar has a
//! glyph (an empty one is drawn as a blank square).

#[test]
fn each_panel_of_the_activity_bar_has_a_glyph() {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    let panels = manifest["contributes"]["panels"].as_array().unwrap();
    for panel in panels.iter().filter(|p| p["location"] != "settings") {
        let icon = panel["icon"].as_str().unwrap_or_default();
        assert!(
            icon.chars().count() == 1
                && ('\u{e000}'..='\u{f8ff}').contains(&icon.chars().next().unwrap()),
            "panel {}: {icon:?} is not one glyph of the Nerd Font",
            panel["id"]
        );
    }
}

#[test]
fn the_manifest_is_one_the_editor_accepts_and_its_panel_goes_with_git() {
    use ardoise_plugin::manifest::{Group, Manifest};
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.json")).unwrap();
    let manifest = Manifest::parse(&text).unwrap();
    assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest.contributes.panels[0].group, Some(Group::Git));
}
