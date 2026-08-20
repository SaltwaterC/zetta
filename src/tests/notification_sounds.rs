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
    assert_eq!(BuiltinSound::parse("zetta-gong"), Some(BuiltinSound::Gong));
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
fn gong_has_a_long_resonant_tail() {
    const SAMPLE_RATE: u32 = 44_100;
    let samples = BuiltinSound::Gong.samples(SAMPLE_RATE);
    let expected_samples = (SAMPLE_RATE as u64 * GONG_DURATION_MS as u64 / 1000) as usize;
    let early_energy = samples[..SAMPLE_RATE as usize / 10]
        .iter()
        .map(|sample| sample * sample)
        .sum::<f32>()
        / (SAMPLE_RATE as f32 / 10.0);
    let tail_energy = samples[SAMPLE_RATE as usize * 3 / 4..]
        .iter()
        .map(|sample| sample * sample)
        .sum::<f32>()
        / (samples.len() - SAMPLE_RATE as usize * 3 / 4) as f32;

    assert_eq!(samples.len(), expected_samples);
    assert!(early_energy > 0.0001);
    assert!(tail_energy > 0.00001);
    assert!(tail_energy < early_energy);
}

#[test]
fn gong_uses_the_available_headroom_without_clipping() {
    let samples = BuiltinSound::Gong.samples(44_100);
    let peak = samples.iter().copied().map(f32::abs).fold(0.0, f32::max);

    assert!((peak - GONG_PEAK_AMPLITUDE).abs() < 0.000_001);
}

#[test]
fn gong_low_body_blooms_after_the_strike() {
    const SAMPLE_RATE: usize = 44_100;
    let samples = BuiltinSound::Gong.samples(SAMPLE_RATE as u32);
    let mut low_passed = Vec::with_capacity(samples.len());
    let coefficient = (-std::f32::consts::TAU * 250.0 / SAMPLE_RATE as f32).exp();
    let mut low = 0.0;
    for sample in &samples {
        low = low * coefficient + sample * (1.0 - coefficient);
        low_passed.push(low);
    }

    let initial_low_energy = mean_square(&low_passed[SAMPLE_RATE / 200..SAMPLE_RATE / 25]);
    let bloomed_low_energy = mean_square(&low_passed[SAMPLE_RATE / 6..SAMPLE_RATE / 3]);

    assert!(bloomed_low_energy > initial_low_energy);
}

#[test]
fn gong_does_not_carry_a_broadband_hiss() {
    const SAMPLE_RATE: usize = 44_100;
    let samples = BuiltinSound::Gong.samples(SAMPLE_RATE as u32);
    let coefficient = (-std::f32::consts::TAU * 4_000.0 / SAMPLE_RATE as f32).exp();
    let mut low_passed = 0.0;
    let mut total_energy = 0.0;
    let mut high_frequency_energy = 0.0;

    for sample in &samples[..SAMPLE_RATE] {
        low_passed = low_passed * coefficient + sample * (1.0 - coefficient);
        total_energy += sample * sample;
        high_frequency_energy += (sample - low_passed).powi(2);
    }

    assert!(high_frequency_energy / total_energy < 0.04);
}

fn mean_square(samples: &[f32]) -> f32 {
    samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32
}

#[test]
fn gong_mulaw_asset_has_the_expected_duration_and_polarity() {
    assert_eq!(GONG_MULAW.len(), 92_610);
    assert_eq!(decode_mulaw(0xff), 0.0);
    assert_eq!(decode_mulaw(0x7f), 0.0);
    assert!(decode_mulaw(0x80) > 0.9);
    assert!(decode_mulaw(0x00) < -0.9);
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
    let path = prepare_macos_builtin_sound(BuiltinSound::Gong, directory.path()).unwrap();
    let wav = std::fs::read(&path).unwrap();
    let data_size = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
    let rendered_samples = BuiltinSound::Gong.samples(44_100).len();
    let trailing_samples = 44_100 * 200 / 1000;

    assert_eq!(path.file_name().unwrap(), "zetta-gong-v1.wav",);
    assert_eq!(data_size, (rendered_samples + trailing_samples) * 2);
    assert!(
        wav[wav.len() - trailing_samples * 2..]
            .iter()
            .all(|byte| *byte == 0)
    );
    assert_eq!(
        prepare_macos_builtin_sound(BuiltinSound::Gong, directory.path()).unwrap(),
        path
    );
}
