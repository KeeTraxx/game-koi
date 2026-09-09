//! Audio output: hands the APU's samples to the host sound device.
//!
//! Like the rest of the frontend, this is outside the emulated machine. The APU
//! produces stereo samples into a queue; this module drains that queue into whatever
//! cpal gives us.
//!
//! # Two clocks that do not agree
//!
//! The emulator runs a frame's worth of instructions and then sleeps, so samples arrive
//! in bursts of about 800 every 16.7 ms. The sound card, meanwhile, asks for samples on
//! its own schedule from its own thread, and its idea of a second is a crystal that has
//! no relationship to ours. A buffer between the two is not optional.
//!
//! The buffer is a plain `Mutex<VecDeque>`. A lock-free ring would be the textbook
//! answer for an audio callback, but the critical section here is a memcpy of a few
//! hundred samples with no allocation inside it, which is short enough that the audio
//! thread is not realistically going to miss a deadline waiting for it.
//!
//! When the buffer runs dry the callback emits silence rather than stalling or repeating
//! the last sample. Underruns are audible as a click, and the honest fix is to keep the
//! emulator fed, not to paper over it here.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};

/// How much audio to hold before dropping the oldest.
///
/// Roughly 100 ms at 48 kHz. Enough to ride out a slow frame; short enough that the lag
/// between seeing a hit and hearing it stays under what anyone notices.
const MAX_BUFFERED: usize = 4800;

/// The shared queue between the emulator thread and cpal's callback.
type SampleBuffer = Arc<Mutex<VecDeque<(f32, f32)>>>;

/// An open output stream, plus the buffer feeding it.
///
/// Dropping this stops the stream — cpal ties playback to the `Stream`'s lifetime, so it
/// has to be kept alive even though nothing calls methods on it.
pub struct Audio {
    buffer: SampleBuffer,
    _stream: Stream,
    /// The rate the device actually chose, which the APU needs in order to resample to
    /// it. Not necessarily the 48 kHz we asked for.
    sample_rate: u32,
    channels: usize,
}

impl Audio {
    /// Opens the default output device.
    ///
    /// Returns `None` rather than an error if there is no usable device: a machine with
    /// no sound card should still run the emulator, just silently.
    pub fn new() -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let default_config = device.default_output_config().ok()?;

        let sample_rate = default_config.sample_rate();
        let channels = default_config.channels() as usize;
        let config = StreamConfig {
            channels: default_config.channels(),
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let buffer: SampleBuffer = Arc::new(Mutex::new(VecDeque::new()));
        let callback_buffer = Arc::clone(&buffer);

        let stream = device
            .build_output_stream(
                config,
                move |output: &mut [f32], _| {
                    let mut queue = callback_buffer.lock().unwrap_or_else(|e| e.into_inner());
                    for frame in output.chunks_mut(channels) {
                        // Silence on underrun. The alternative — holding the last
                        // sample — turns a click into a buzz, which is worse.
                        let (left, right) = queue.pop_front().unwrap_or((0.0, 0.0));
                        for (i, sample) in frame.iter_mut().enumerate() {
                            *sample = if i % 2 == 0 { left } else { right };
                        }
                    }
                },
                move |err| eprintln!("audio error: {err}"),
                None,
            )
            .ok()?;

        stream.play().ok()?;

        Some(Audio {
            buffer,
            _stream: stream,
            sample_rate,
            channels,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Hands a batch of samples to the audio thread.
    ///
    /// Drops the oldest when the buffer is full, which happens if the emulator is
    /// running ahead of the sound card. Dropping the *newest* would be easier but would
    /// mean the audio falls permanently further behind the picture.
    pub fn queue(&self, samples: &[(f32, f32)]) {
        if samples.is_empty() {
            return;
        }
        let Ok(mut queue) = self.buffer.lock() else {
            return;
        };
        queue.extend(samples.iter().copied());
        while queue.len() > MAX_BUFFERED {
            queue.pop_front();
        }
    }

    /// How many host channels the device wants. Reported so the caller can say so.
    pub fn channels(&self) -> usize {
        self.channels
    }
}
