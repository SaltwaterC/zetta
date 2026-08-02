use super::*;

#[test]
fn parse_recognizes_exact_names_and_rejects_unknown_values() {
    assert_eq!(
        BuiltinSound::parse("zetta-default"),
        Some(BuiltinSound::Default)
    );
    assert_eq!(BuiltinSound::parse("zetta-ok"), Some(BuiltinSound::Ok));
    assert_eq!(
        BuiltinSound::parse("zetta-alarm"),
        Some(BuiltinSound::Alarm)
    );
    assert_eq!(BuiltinSound::parse("Zetta-Default"), None);
    assert_eq!(BuiltinSound::parse("bell"), None);
    assert_eq!(BuiltinSound::parse(""), None);
}

#[test]
fn every_builtin_sound_round_trips_through_its_name() {
    for sound in BuiltinSound::ALL {
        assert_eq!(BuiltinSound::parse(sound.name()), Some(sound));
    }
}

#[test]
fn rendered_samples_are_finite_and_within_the_peak_amplitude() {
    const SAMPLE_RATE: u32 = 44_100;
    for sound in BuiltinSound::ALL {
        let samples = sound.samples(SAMPLE_RATE);
        assert!(!samples.is_empty());
        for sample in &samples {
            assert!(sample.is_finite());
            assert!(
                sample.abs() <= 0.31,
                "sample {sample} exceeds the expected peak amplitude"
            );
        }
    }
}

#[test]
fn rendered_sample_count_matches_the_notes_total_duration() {
    const SAMPLE_RATE: u32 = 44_100;
    let expected_ms: u32 = BuiltinSound::Alarm
        .notes()
        .iter()
        .map(|note| note.duration_ms)
        .sum();
    let expected_samples = (SAMPLE_RATE as u64 * expected_ms as u64 / 1000) as usize;
    assert_eq!(
        BuiltinSound::Alarm.samples(SAMPLE_RATE).len(),
        expected_samples
    );
}

#[test]
fn silent_notes_render_as_zero_amplitude() {
    let silence = render(&[Note::silence(10)], 44_100);
    assert!(silence.iter().all(|sample| *sample == 0.0));
}

// Regression test: the fade-out envelope must reach exactly zero at the true
// last sample of a tone. A stream is torn down right after this point, so
// anything above zero here is audible as a click/pop at the end of playback.
#[test]
fn rendered_tone_notes_start_and_end_at_exactly_zero_amplitude() {
    for sound in BuiltinSound::ALL {
        let samples = sound.samples(44_100);
        assert_eq!(
            *samples.first().unwrap(),
            0.0,
            "{sound:?} does not start at zero"
        );
        assert_eq!(
            *samples.last().unwrap(),
            0.0,
            "{sound:?} does not end at zero"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_wav_output_is_pcm_with_the_rendered_sample_count() {
    let samples = vec![0.0, 0.5, -0.5];
    let mut wav = Vec::new();
    write_wav(&mut wav, 44_100, &samples).unwrap();

    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
    assert_eq!(wav.len(), 44 + samples.len() * 2);
    assert_eq!(i16::from_le_bytes(wav[44..46].try_into().unwrap()), 0);
    assert_eq!(i16::from_le_bytes(wav[46..48].try_into().unwrap()), 16_384);
    assert_eq!(i16::from_le_bytes(wav[48..50].try_into().unwrap()), -16_384);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_builtin_sound_is_cached_with_trailing_silence() {
    let directory = tempfile::tempdir().unwrap();
    let path = prepare_macos_builtin_sound(BuiltinSound::Alarm, directory.path()).unwrap();
    let wav = std::fs::read(&path).unwrap();
    let data_size = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
    let rendered_samples = BuiltinSound::Alarm.samples(44_100).len();
    let trailing_samples = 44_100 * 200 / 1000;

    assert_eq!(path.file_name().unwrap(), "zetta-alarm-v1.wav",);
    assert_eq!(data_size, (rendered_samples + trailing_samples) * 2);
    assert!(
        wav[wav.len() - trailing_samples * 2..]
            .iter()
            .all(|byte| *byte == 0)
    );
    assert_eq!(
        prepare_macos_builtin_sound(BuiltinSound::Alarm, directory.path()).unwrap(),
        path
    );
}
