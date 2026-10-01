use super::*;

#[test]
fn every_tone_has_its_own_colour_and_icon() {
    let colors = ThemeColors::dark();
    let status = StatusColors::dark();
    let tones = [Tone::Error, Tone::Warning, Tone::Info, Tone::Success];
    for (index, tone) in tones.iter().enumerate() {
        for other in &tones[index + 1..] {
            assert_ne!(tone.icon(), other.icon(), "{tone:?} and {other:?}");
            assert_ne!(
                tone.color(&colors, &status),
                other.color(&colors, &status),
                "{tone:?} and {other:?}"
            );
        }
    }
}

#[test]
fn an_error_is_never_shown_in_the_plain_text_colour() {
    let colors = ThemeColors::dark();
    let status = StatusColors::dark();

    assert_ne!(Tone::Error.color(&colors, &status), colors.text);
}
