//! Capture through cpal: WASAPI loopback on Windows (an input stream opened
//! on the default *output* device), the default input device elsewhere.

use crate::{CaptureError, Target, push_mono};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::Producer;

pub struct Stream {
    _stream: cpal::Stream,
    sample_rate: u32,
}

impl Stream {
    pub fn start(_target: &Target, mut sink: Producer<f32>) -> Result<Stream, CaptureError> {
        let host = cpal::default_host();
        // Windows: loopback of what the speakers play. Elsewhere: the default
        // input, which on Linux can be pointed at an output's monitor.
        #[cfg(windows)]
        let (device, config) = {
            let d = host
                .default_output_device()
                .ok_or_else(|| CaptureError::Failed("no output device".into()))?;
            let c = d
                .default_output_config()
                .map_err(|e| CaptureError::Failed(e.to_string()))?;
            (d, c)
        };
        #[cfg(not(windows))]
        let (device, config) = {
            let d = host.default_input_device().ok_or_else(|| {
                CaptureError::Unsupported("no input device to capture from".into())
            })?;
            let c = d
                .default_input_config()
                .map_err(|e| CaptureError::Failed(e.to_string()))?;
            (d, c)
        };
        if config.sample_format() != cpal::SampleFormat::F32 {
            return Err(CaptureError::Unsupported(format!(
                "sample format {} (only f32 is handled)",
                config.sample_format()
            )));
        }
        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate();
        let stream = device
            .build_input_stream(
                config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    push_mono(&mut sink, data, channels)
                },
                |_err| {},
                None,
            )
            .map_err(|e| CaptureError::Failed(e.to_string()))?;
        stream
            .play()
            .map_err(|e| CaptureError::Failed(e.to_string()))?;
        Ok(Stream {
            _stream: stream,
            sample_rate,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
