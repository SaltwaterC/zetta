use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// The built-in notification tones bundled with Zetta. Unlike a system sound
/// name (which the OS notification server resolves against its own sound
/// theme and may not play at all), these are synthesized and played directly
/// by Zetta, so they work identically regardless of the host's audio setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltinSound {
    Default,
    Ok,
    Alarm,
}

impl BuiltinSound {
    pub(crate) const ALL: [Self; 3] = [Self::Default, Self::Ok, Self::Alarm];

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sound| sound.name() == name)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Default => "zetta-default",
            Self::Ok => "zetta-ok",
            Self::Alarm => "zetta-alarm",
        }
    }

    fn notes(self) -> &'static [Note] {
        static DEFAULT_NOTES: [Note; 1] = [Note::tone(880.0, 180)];
        static OK_NOTES: [Note; 3] = [
            Note::tone(659.25, 110),
            Note::silence(20),
            Note::tone(880.0, 160),
        ];
        static ALARM_NOTES: [Note; 5] = [
            Note::tone(1046.5, 90),
            Note::silence(70),
            Note::tone(1046.5, 90),
            Note::silence(70),
            Note::tone(1046.5, 90),
        ];
        match self {
            Self::Default => &DEFAULT_NOTES,
            Self::Ok => &OK_NOTES,
            Self::Alarm => &ALARM_NOTES,
        }
    }

    fn samples(self, sample_rate: u32) -> Vec<f32> {
        render(self.notes(), sample_rate)
    }

    pub(crate) fn play(self) -> Result<()> {
        // Output is buffered: the callback fills audio somewhat ahead of when
        // it is physically played, so hardware is typically still draining
        // real audio when the callback writes its last real sample. Padding
        // the buffer with trailing silence keeps the stream alive (still
        // outputting true silence) until that has had time to happen, so
        // dropping the stream afterwards does not truncate audible content
        // and produce a click.
        const TRAILING_SILENCE_SECONDS: f32 = 0.2;
        const TEARDOWN_GRACE: Duration = Duration::from_millis(50);

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("no audio output device is available")?;
        let config = device
            .default_output_config()
            .context("could not determine the audio output configuration")?;
        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate().0;
        let mut samples = self.samples(sample_rate);
        samples.extend(std::iter::repeat_n(
            0.0,
            (sample_rate as f32 * TRAILING_SILENCE_SECONDS) as usize,
        ));
        let duration = Duration::from_secs_f64(samples.len() as f64 / sample_rate as f64);
        let stream_config: cpal::StreamConfig = config.clone().into();
        let done = Arc::new((Mutex::new(false), Condvar::new()));

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                build_stream::<f32>(&device, &stream_config, channels, samples, done.clone())?
            }
            cpal::SampleFormat::I16 => {
                build_stream::<i16>(&device, &stream_config, channels, samples, done.clone())?
            }
            cpal::SampleFormat::U16 => {
                build_stream::<u16>(&device, &stream_config, channels, samples, done.clone())?
            }
            other => anyhow::bail!("unsupported audio output sample format {other:?}"),
        };
        stream.play().context("failed to start audio playback")?;

        let (lock, condvar) = &*done;
        let finished = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_finished, result) = condvar
            .wait_timeout_while(
                finished,
                duration + Duration::from_millis(500),
                |finished| !*finished,
            )
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        anyhow::ensure!(!result.timed_out(), "audio playback did not finish in time");
        std::thread::sleep(TEARDOWN_GRACE);
        Ok(())
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    samples: Vec<f32>,
    done: Arc<(Mutex<bool>, Condvar)>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut position = 0usize;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], _| {
                for frame in output.chunks_mut(channels) {
                    let value = samples.get(position).copied().unwrap_or(0.0);
                    let converted = T::from_sample(value);
                    for sample in frame.iter_mut() {
                        *sample = converted;
                    }
                    position += 1;
                    if position >= samples.len() {
                        let (lock, condvar) = &*done;
                        let mut finished =
                            lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        *finished = true;
                        condvar.notify_one();
                    }
                }
            },
            |error| eprintln!("audio output error: {error}"),
            None,
        )
        .context("failed to build the audio output stream")
}

struct Note {
    frequency_hz: f32,
    duration_ms: u32,
}

impl Note {
    const fn tone(frequency_hz: f32, duration_ms: u32) -> Self {
        Self {
            frequency_hz,
            duration_ms,
        }
    }

    const fn silence(duration_ms: u32) -> Self {
        Self {
            frequency_hz: 0.0,
            duration_ms,
        }
    }
}

// Each note is rendered with a short linear fade in/out to avoid the audible
// clicks a hard-edged sine wave produces at its start and end.
fn render(notes: &[Note], sample_rate: u32) -> Vec<f32> {
    const PEAK_AMPLITUDE: f32 = 0.3;
    const FADE_SECONDS: f32 = 0.005;

    let mut samples = Vec::new();
    for note in notes {
        let sample_count = (sample_rate as u64 * note.duration_ms as u64 / 1000) as usize;
        if note.frequency_hz <= 0.0 {
            samples.extend(std::iter::repeat_n(0.0, sample_count));
            continue;
        }
        let fade = ((sample_rate as f32 * FADE_SECONDS) as usize).min(sample_count / 2);
        for index in 0..sample_count {
            let t = index as f32 / sample_rate as f32;
            let envelope = if index < fade {
                index as f32 / fade as f32
            } else if index >= sample_count - fade {
                (sample_count - 1 - index) as f32 / fade as f32
            } else {
                1.0
            };
            let value = (2.0 * std::f32::consts::PI * note.frequency_hz * t).sin()
                * envelope
                * PEAK_AMPLITUDE;
            samples.push(value);
        }
    }
    samples
}

#[cfg(test)]
#[path = "tests/notification_sounds.rs"]
mod tests;
