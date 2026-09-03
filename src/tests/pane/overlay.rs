use super::*;

#[test]
fn overlay_style_picker_percent_snaps_to_five_percent_steps() {
    let default = OverlayStylePicker::percent_for_opacity(None);
    assert_eq!(DEFAULT_OVERLAY_OPACITY, 0.85);
    assert_eq!(default, 85);

    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(0.)), 0);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(0.5)), 50);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(1.)), 100);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(0.37)), 35);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(1.4)), 100);
    assert_eq!(OverlayStylePicker::percent_for_opacity(Some(-0.2)), 0);
}

#[test]
fn overlay_style_picker_preset_cursor_clamps_to_the_six_column_grid() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::ColorPresets,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };

    assert_eq!(OVERLAY_COLOR_PRESET_COLUMNS, 6);
    picker.move_preset_cursor(0, -1);
    assert_eq!(picker.preset_index, 0);
    picker.move_preset_cursor(-1, 0);
    assert_eq!(picker.preset_index, 0);

    picker.move_preset_cursor(0, 1);
    assert_eq!(picker.preset_index, 1);
    picker.set_preset_index(5);
    picker.move_preset_cursor(0, 1);
    assert_eq!(picker.preset_index, 5);
    picker.move_preset_cursor(1, 0);
    assert_eq!(picker.preset_index, 11);
    picker.move_preset_cursor(1, 0);
    assert_eq!(picker.preset_index, 11);

    picker.move_preset_cursor(0, -1);
    assert_eq!(picker.preset_index, 10);
    picker.move_preset_cursor(-1, 0);
    assert_eq!(picker.preset_index, 4);
    picker.set_preset_index(6);
    picker.move_preset_cursor(0, -1);
    assert_eq!(picker.preset_index, 6);
    picker.set_preset_index(usize::MAX);
    assert_eq!(picker.preset_index, 11);
}

#[test]
fn overlay_style_picker_preset_cursor_starts_at_a_matching_preset_or_first() {
    let red = overlay_color_from_hex("#ff0000").unwrap();
    assert_eq!(OverlayStylePicker::preset_index_for_color(red), 3);

    let custom = overlay_color_from_hex("#123456").unwrap();
    assert_eq!(OverlayStylePicker::preset_index_for_color(custom), 0);
}

#[test]
fn overlay_color_presets_parse_to_their_canonical_opaque_values() {
    let expected = [
        ("black", "#000000"),
        ("white", "#ffffff"),
        ("gray", "#808080"),
        ("red", "#ff0000"),
        ("orange", "#ffa500"),
        ("yellow", "#ffff00"),
        ("green", "#008000"),
        ("cyan", "#00ffff"),
        ("blue", "#0000ff"),
        ("purple", "#800080"),
        ("magenta", "#ff00ff"),
        ("pink", "#ffc0cb"),
    ];

    assert_eq!(OVERLAY_COLOR_PRESETS.len(), expected.len());
    for (preset, (name, hex)) in OVERLAY_COLOR_PRESETS.iter().zip(expected) {
        assert_eq!((preset.name, preset.hex), (name, hex));
        let color = overlay_color_from_value(preset.name).expect("preset should parse");
        assert_eq!(overlay_color_to_hex(color), hex);
        assert!((color.a - 1.).abs() < f32::EPSILON);
    }

    assert_eq!(
        overlay_color_to_hex(overlay_color_from_value("  ReD  ").unwrap()),
        "#ff0000"
    );
    assert_eq!(
        overlay_color_to_hex(overlay_color_from_value("YELLOW").unwrap()),
        "#ffff00"
    );
}

#[test]
fn overlay_color_value_parser_preserves_hex_forms_and_rejects_invalid_values() {
    for (value, expected_hex, expected_alpha) in [
        ("f0a", "#ff00aa", 1.),
        ("#f0a8", "#ff00aa", 136. / 255.),
        ("112233", "#112233", 1.),
        ("#11223344", "#112233", 68. / 255.),
    ] {
        let color = overlay_color_from_value(value).expect("hex colour should parse");
        assert_eq!(overlay_color_to_hex(color), expected_hex);
        assert!((color.a - expected_alpha).abs() < 1e-6);
    }

    for value in ["", "grey", "not-a-color", "#12", "#ggg", "#12345"] {
        assert!(
            overlay_color_from_value(value).is_none(),
            "expected {value:?} to be rejected"
        );
    }
}

#[test]
fn overlay_style_picker_round_trips_hsl_to_hsv() {
    for hsla in [
        gpui::hsla(0., 1., 0.5, 1.),   // red
        gpui::hsla(0.5, 1., 0.5, 1.),  // cyan
        gpui::hsla(0.3, 0.4, 0.2, 1.), // dark olive
        gpui::hsla(0., 0., 1., 1.),    // white
        gpui::hsla(0., 0., 0., 1.),    // black
    ] {
        let (hue, saturation, value) = hsla_to_hsv(hsla);
        let back = hsv_to_hsla(hue, saturation, value);
        assert!((back.h - hsla.h).abs() < 1e-4);
        assert!((back.s - hsla.s).abs() < 1e-4);
        assert!((back.l - hsla.l).abs() < 1e-4);
    }
}

#[test]
fn overlay_style_picker_hex_field_colors_the_selection() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };
    picker.refresh_hex();
    assert_eq!(picker.hex_buffer, "#ffffff");

    // A complete buffer is replaced by a fresh entry instead of growing.
    assert!(!picker.hex_input('#'));
    assert!(!picker.hex_input('f'));
    assert!(!picker.hex_input('0'));
    assert_eq!(picker.hex_buffer, "#f0");

    picker.refresh_hex();
    for ch in ['f', 'f', '0', '0', '0', '0'] {
        picker.hex_input(ch);
    }
    assert_eq!(picker.hex_buffer, "#ff0000");
    let color = picker.color();
    assert!(color.h.abs() < 1e-3 || (color.h - 1.).abs() < 1e-3);
    assert!(color.s > 0.99);
}

#[test]
fn overlay_style_picker_preset_selection_keeps_hsv_and_hex_editing_live() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };

    let orange = OVERLAY_COLOR_PRESETS
        .iter()
        .find(|preset| preset.name == "orange")
        .copied()
        .unwrap();
    picker.set_color_preset(orange);
    assert_eq!(picker.preset_index, 4);
    assert_eq!(picker.hex_buffer, orange.hex);
    assert_eq!(overlay_color_to_hex(picker.color()), orange.hex);

    picker.adjust_value(-0.2);
    assert!(picker.value < 1.);
    assert_ne!(picker.hex_buffer, orange.hex);
    assert_eq!(picker.preset_index, 4);

    picker.hex_buffer = "#".to_owned();
    for digit in ['0', '0', '8', '0', '0', '0'] {
        picker.hex_input(digit);
    }
    assert_eq!(picker.hex_buffer, "#008000");
    assert_eq!(overlay_color_to_hex(picker.color()), "#008000");
    assert_eq!(picker.preset_index, 4);
}

#[test]
fn overlay_style_picker_hex_accepts_three_digit_codes() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };
    picker.refresh_hex();

    // `#f00` commits as soon as the third digit lands and keeps its literal
    // buffer form so a longer code can keep being typed.
    assert!(!picker.hex_input('f'));
    assert!(!picker.hex_input('0'));
    assert!(picker.hex_input('0'));
    assert_eq!(picker.hex_buffer, "#f00");
    let red = picker.color();
    assert!(red.h.abs() < 1e-3 || (red.h - 1.).abs() < 1e-3);
    assert!(red.s > 0.99);

    // A six-digit code typed straight through is not interrupted by the
    // short-form commit.
    picker.refresh_hex();
    for ch in ['f', 'f', '0', '0', '0', '0'] {
        picker.hex_input(ch);
    }
    assert_eq!(picker.hex_buffer, "#ff0000");
    let red = picker.color();
    assert!(red.h.abs() < 1e-3 || (red.h - 1.).abs() < 1e-3);
    assert!(red.s > 0.99);
}

#[test]
fn overlay_style_picker_hex_backspace_keeps_the_hash() {
    let mut picker = OverlayStylePicker {
        pane_id: 2,
        section: OverlayPickerSection::Color,
        font_size: OverlayFontSize::DEFAULT,
        original_font_size: None,
        hue: 0.,
        saturation: 0.,
        value: 1.,
        original_color: None,
        preset_index: 0,
        opacity_percent: 85,
        original_opacity: None,
        hex_buffer: String::new(),
    };
    picker.refresh_hex();
    picker.hex_input('a');
    picker.hex_input('b');
    picker.hex_input('c');

    // Backspace keeps the leading `#`; repeated backspaces never clear it.
    assert!(!picker.hex_backspace());
    picker.hex_backspace();
    picker.hex_backspace();
    assert_eq!(picker.hex_buffer, "#");
    assert!(!picker.hex_backspace());
    assert_eq!(picker.hex_buffer, "#");
}

#[test]
fn overlay_style_picker_seeds_a_pleasant_hue_for_achromatic_colors() {
    let white = gpui::hsla(0., 0., 1., 1.);
    let (hue, saturation, value) = overlay_picker_hsv_from_hsla(white);
    assert_eq!(hue, DEFAULT_PICKER_HUE);
    assert!(saturation < 0.05);
    assert!(value > 0.95);

    let blue = gpui::hsla(0.6, 1., 0.5, 1.);
    let (hue, saturation, value) = overlay_picker_hsv_from_hsla(blue);
    assert!((hue - 0.6).abs() < 1e-3);
    assert!(saturation > 0.99);
    assert!(value > 0.4);
}

#[test]
fn overlay_font_size_steps_wrap_around_the_ends() {
    assert_eq!(
        OverlayFontSize::Small.step(-1),
        OverlayFontSize::ExtraExtraExtraLarge
    );
    assert_eq!(
        OverlayFontSize::ExtraExtraExtraLarge.step(1),
        OverlayFontSize::Small
    );
    assert_eq!(
        OverlayFontSize::ExtraLarge.step(1),
        OverlayFontSize::ExtraExtraLarge
    );
    assert_eq!(OverlayFontSize::ExtraLarge.step(-2), OverlayFontSize::Base);
}

#[test]
fn overlay_picker_section_steps_wrap_around_the_ends() {
    assert_eq!(
        OverlayPickerSection::FontSize.step(-1),
        OverlayPickerSection::ColorPresets
    );
    assert_eq!(
        OverlayPickerSection::FontSize.step(1),
        OverlayPickerSection::Opacity
    );
    assert_eq!(
        OverlayPickerSection::Opacity.step(1),
        OverlayPickerSection::Color
    );
    assert_eq!(
        OverlayPickerSection::Color.step(1),
        OverlayPickerSection::ColorPresets
    );
    assert_eq!(
        OverlayPickerSection::ColorPresets.step(1),
        OverlayPickerSection::FontSize
    );
    assert_eq!(
        OverlayPickerSection::Opacity.step(-1),
        OverlayPickerSection::FontSize
    );
    assert_eq!(
        OverlayPickerSection::Color.step(-1),
        OverlayPickerSection::Opacity
    );
    assert_eq!(
        OverlayPickerSection::ColorPresets.step(-1),
        OverlayPickerSection::Color
    );
}
