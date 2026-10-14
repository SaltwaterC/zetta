use super::*;

#[test]
fn theme_colors_report_as_console_defaults_and_dim_as_their_base_color() {
    assert_eq!(
        pen_color(Color::Named(NamedColor::Foreground)),
        PenColor::Default
    );
    assert_eq!(
        pen_color(Color::Named(NamedColor::Background)),
        PenColor::Default
    );
    assert_eq!(
        pen_color(Color::Named(NamedColor::BrightRed)),
        PenColor::Indexed(9)
    );
    assert_eq!(
        pen_color(Color::Named(NamedColor::DimRed)),
        PenColor::Indexed(1)
    );
    assert_eq!(pen_color(Color::Indexed(200)), PenColor::Indexed(200));
    assert_eq!(
        pen_color(Color::Spec(alacritty_terminal::vte::ansi::Rgb {
            r: 1,
            g: 2,
            b: 3
        })),
        PenColor::Rgb(1, 2, 3)
    );
}

#[test]
fn the_pane_palette_reaches_the_console_in_attribute_order() {
    let console = console_palette(ConsolePalette {
        foreground_index: 15,
        ..Default::default()
    });
    // Legacy index 1 is FOREGROUND_BLUE: ANSI blue (0, 0, 128) as COLORREF.
    assert_eq!(console.colors[1], 0x80_0000);
    // Legacy index 4 is FOREGROUND_RED: ANSI red (128, 0, 0).
    assert_eq!(console.colors[4], 0x00_0080);
    assert_eq!(console.attributes, 0x0f);
}
