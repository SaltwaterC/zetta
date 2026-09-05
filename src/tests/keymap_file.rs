use super::*;

#[test]
fn an_empty_file_parses_to_no_sections() {
    let keymap = KeymapFile::parse("").expect("empty content is not an error");
    assert_eq!(keymap.sections().count(), 0);
    let keymap = KeymapFile::parse("   \n  ").expect("blank content is not an error");
    assert_eq!(keymap.sections().count(), 0);
}

/// Keymap files carry `//` comments, which `serde_json` rejects outright. The
/// bundled keymap has 45 of them, so a strict parser would load none of it.
#[test]
fn comments_are_accepted() {
    let keymap = KeymapFile::parse(
        r#"[
            // a leading comment
            {
                "context": "Terminal",
                "bindings": {
                    "ctrl-a": "zetta::NewTab" // a trailing comment
                }
            }
        ]"#,
    )
    .expect("comments must parse");
    let section = keymap.sections().next().expect("one section");
    assert_eq!(section.context, "Terminal");
    assert_eq!(section.bindings().count(), 1);
}

#[test]
fn a_section_without_a_context_is_allowed() {
    let keymap = KeymapFile::parse(r#"[{ "bindings": { "ctrl-a": "zetta::NewTab" } }]"#).unwrap();
    let section = keymap.sections().next().unwrap();
    assert_eq!(section.context, "");
    assert_eq!(section.bindings().count(), 1);
}

#[test]
fn a_section_without_bindings_yields_none() {
    let keymap = KeymapFile::parse(r#"[{ "context": "Terminal" }]"#).unwrap();
    assert_eq!(keymap.sections().next().unwrap().bindings().count(), 0);
}

/// Binding order decides precedence, so the map has to preserve file order
/// rather than sort or hash it.
#[test]
fn binding_order_is_preserved() {
    let keymap =
        KeymapFile::parse(r#"[{ "bindings": { "ctrl-c": "a", "ctrl-a": "b", "ctrl-b": "c" } }]"#)
            .unwrap();
    let order: Vec<&str> = keymap
        .sections()
        .next()
        .unwrap()
        .bindings()
        .map(|(keystrokes, _)| keystrokes.as_str())
        .collect();
    assert_eq!(order, vec!["ctrl-c", "ctrl-a", "ctrl-b"]);
}

#[test]
fn parse_action_reads_each_accepted_shape() {
    let keymap = KeymapFile::parse(
        r#"[{
            "bindings": {
                "ctrl-a": "zetta::NewTab",
                "ctrl-b": ["zetta::OpenProfile", 2],
                "ctrl-c": null
            }
        }]"#,
    )
    .unwrap();
    let mut bindings = keymap.sections().next().unwrap().bindings();

    let (_, plain) = bindings.next().unwrap();
    let (name, input) = KeymapFile::parse_action(plain).unwrap().unwrap();
    assert_eq!(name, "zetta::NewTab");
    assert!(input.is_none());

    let (_, with_input) = bindings.next().unwrap();
    let (name, input) = KeymapFile::parse_action(with_input).unwrap().unwrap();
    assert_eq!(name, "zetta::OpenProfile");
    assert_eq!(input.unwrap().as_u64(), Some(2));

    // An explicit `null` binds nothing, which is how a keymap suppresses a
    // default binding without naming a replacement.
    let (_, null) = bindings.next().unwrap();
    assert!(KeymapFile::parse_action(null).unwrap().is_none());
}

#[test]
fn parse_action_rejects_malformed_shapes() {
    let keymap = KeymapFile::parse(
        r#"[{
            "bindings": {
                "ctrl-a": ["zetta::NewTab"],
                "ctrl-b": [2, "zetta::NewTab"],
                "ctrl-c": 7
            }
        }]"#,
    )
    .unwrap();
    for (_, action) in keymap.sections().next().unwrap().bindings() {
        assert!(
            KeymapFile::parse_action(action).is_err(),
            "malformed action should be rejected"
        );
    }
}

#[test]
fn a_malformed_file_reports_a_parse_failure() {
    assert!(KeymapFile::parse("{ not a keymap").is_err());
}

/// Every binding in Zetta's own documented keymap must resolve to a registered
/// action. This is the regression test for owning the loader: a keymap that
/// silently drops bindings looks exactly like one that works until someone
/// presses the key.
#[gpui::test]
fn zettas_example_keymap_binds_every_action_it_names(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let content = include_str!("../../keymap.example.json");
        match KeymapFile::load(content, cx) {
            KeymapFileLoadResult::Success { key_bindings } => {
                assert!(
                    key_bindings.len() > 20,
                    "expected the example keymap to bind many keys, got {}",
                    key_bindings.len()
                );
            }
            KeymapFileLoadResult::SomeFailedToLoad { error_message, .. } => {
                panic!("every action in keymap.example.json should resolve: {error_message}")
            }
            KeymapFileLoadResult::JsonParseFailure { error } => {
                panic!("keymap.example.json should parse: {error:#}")
            }
        }
    });
}

/// The bundled keymap is what binds the shared `menu::` actions that Zetta's
/// pickers and context menus dispatch. Partial failure is expected — it names
/// Zed actions Zetta does not register — but it must still yield those.
#[gpui::test]
fn the_bundled_keymap_still_binds_the_shared_menu_actions(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let content = include_str!("../../assets/keymaps/default-linux.json");
        let key_bindings = match KeymapFile::load(content, cx) {
            KeymapFileLoadResult::Success { key_bindings }
            | KeymapFileLoadResult::SomeFailedToLoad { key_bindings, .. } => key_bindings,
            KeymapFileLoadResult::JsonParseFailure { error } => {
                panic!("the bundled keymap should parse: {error:#}")
            }
        };
        let bound: std::collections::HashSet<&str> = key_bindings
            .iter()
            .map(|binding| binding.action().name())
            .collect();
        for action in [
            "menu::Confirm",
            "menu::Cancel",
            "menu::SelectNext",
            "menu::SelectPrevious",
        ] {
            assert!(
                bound.contains(action),
                "the bundled keymap must still bind {action}; bound {} actions",
                bound.len()
            );
        }
    });
}
