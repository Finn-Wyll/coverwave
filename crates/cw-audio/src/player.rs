use crate::analysis::AnalyserHandle;
use crate::reader::{OUT_CHANNELS, TrackReader};
use crate::{AudioError, Track};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use cw_core::{AudioFrame, Queue, Repeat};
use rtrb::{Consumer, Producer, RingBuffer};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Seconds of audio decoded ahead. Pause and volume act in the callback, and
/// seeks flush the buffer, so this only costs memory, never latency.
const BUFFER_SECS: f64 = 0.75;
/// Seconds of mono audio the analysis tap can hold. The analyser drains it
/// every few milliseconds; if it ever falls behind, the callback drops
/// samples rather than wait.
const TAP_SECS: f64 = 0.25;
/// "Previous" restarts the current track if more than this has played.
const PREV_RESTART_SECS: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    Stopped,
}

/// What the UI needs each frame.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub status: Status,
    /// Index into [`Player::tracks`] of the track being heard.
    pub track: Option<usize>,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    /// 0..=100
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// Track indices in play order.
    pub order: Vec<usize>,
    pub last_error: Option<String>,
}

impl Snapshot {
    fn empty(status: Status, volume: u8) -> Snapshot {
        Snapshot {
            status,
            track: None,
            position_secs: 0.0,
            duration_secs: None,
            volume,
            shuffle: false,
            repeat: Repeat::Off,
            order: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Debug)]
enum Command {
    /// A new track list and queue, starting at `start` from `secs`.
    Replace {
        tracks: Vec<Track>,
        start: usize,
        secs: f64,
        generation: u64,
    },
    /// One more track at the end of the list and the queue.
    Append(Track),
    JumpTo(usize),
    Next {
        from: usize,
    },
    Prev {
        from: usize,
        position: f64,
    },
    Seek {
        track: usize,
        secs: f64,
    },
    SetShuffle(bool),
    SetRepeat(Repeat),
}

/// Marks where in the output stream a track (or a seek) begins.
#[derive(Debug, Clone, Copy)]
struct Boundary {
    start_frame: u64,
    track: usize,
    offset_secs: f64,
    duration_secs: Option<f64>,
}

#[derive(Default)]
struct DecoderState {
    boundaries: VecDeque<Boundary>,
    /// Output frame at which playback runs out (queue finished).
    ended_at: Option<u64>,
    order: Vec<usize>,
    shuffle: bool,
    repeat: Repeat,
    last_error: Option<String>,
    /// Bumped by every `Replace`; snapshots from an older list are ignored.
    generation: u64,
}

struct Shared {
    paused: AtomicBool,
    /// Linear gain as f32 bits.
    gain: AtomicU32,
    /// Output frames consumed by the callback (played or flushed).
    consumed: AtomicU64,
    /// Decoder asks the callback to discard everything buffered.
    flush: AtomicBool,
    out_rate: u32,
    state: Mutex<DecoderState>,
}

pub struct Player {
    /// The UI's copy of the decoder's track list, kept in step by sending
    /// every change as a command.
    tracks: Vec<Track>,
    generation: u64,
    shared: Arc<Shared>,
    commands: Sender<Command>,
    volume: u8,
    analysis: AnalyserHandle,
    // Dropping the stream stops audio; keep it alive with the player.
    _stream: cpal::Stream,
}

impl Player {
    /// Opens the default output device and starts playing `tracks` from
    /// `start`. An empty list gives a player that is simply stopped.
    pub fn new(tracks: Vec<Track>, start: usize) -> Result<Player, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| AudioError::Device("no output device found".into()))?;
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let out_rate = config.sample_rate;
        let capacity = (out_rate as f64 * BUFFER_SECS) as usize * OUT_CHANNELS;
        let (producer, consumer) = RingBuffer::<f32>::new(capacity);
        let (tap, tap_out) = RingBuffer::<f32>::new((out_rate as f64 * TAP_SECS) as usize);

        let volume = 80;
        let shared = Arc::new(Shared {
            paused: AtomicBool::new(false),
            gain: AtomicU32::new(volume_to_gain(volume).to_bits()),
            consumed: AtomicU64::new(0),
            flush: AtomicBool::new(false),
            out_rate,
            state: Mutex::new(DecoderState::default()),
        });

        let stream = match format {
            SampleFormat::F32 => {
                build_stream::<f32>(&device, &config, consumer, tap, shared.clone())
            }
            SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, consumer, tap, shared.clone())
            }
            SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, consumer, tap, shared.clone())
            }
            SampleFormat::I32 => {
                build_stream::<i32>(&device, &config, consumer, tap, shared.clone())
            }
            SampleFormat::F64 => {
                build_stream::<f64>(&device, &config, consumer, tap, shared.clone())
            }
            other => Err(AudioError::Device(format!(
                "unsupported sample format {other}"
            ))),
        }?;
        stream
            .play()
            .map_err(|e| AudioError::Device(e.to_string()))?;

        let (tx, rx) = mpsc::channel();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5EED);
        let queue = Queue::new(tracks.len(), start, false, seed);
        let mut decoder = Decoder::new(tracks.clone(), shared.clone(), producer, rx, queue);
        decoder.seed = seed;
        thread::Builder::new()
            .name("cw-decoder".into())
            .spawn(move || decoder.run())
            .map_err(|e| AudioError::Device(format!("decoder thread: {e}")))?;

        let analysis = AnalyserHandle::spawn(tap_out, out_rate)
            .map_err(|e| AudioError::Device(format!("analyser thread: {e}")))?;

        Ok(Player {
            tracks,
            generation: 0,
            shared,
            commands: tx,
            volume,
            analysis,
            _stream: stream,
        })
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn snapshot(&self) -> Snapshot {
        let consumed = self.shared.consumed.load(Ordering::Acquire);
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.generation != self.generation {
            // The decoder has not picked up the new list yet.
            return Snapshot::empty(Status::Stopped, self.volume);
        }
        // Drop boundaries that have been fully passed.
        while st.boundaries.len() > 1 && st.boundaries[1].start_frame <= consumed {
            st.boundaries.pop_front();
        }
        let current = st
            .boundaries
            .front()
            .copied()
            .filter(|b| b.start_frame <= consumed);
        let ended = st.ended_at.is_some_and(|e| consumed >= e);
        let status = if ended || current.is_none() {
            Status::Stopped
        } else if self.shared.paused.load(Ordering::Relaxed) {
            Status::Paused
        } else {
            Status::Playing
        };
        let (track, position_secs, duration_secs) = match current {
            Some(b) => {
                let played = consumed.min(st.ended_at.unwrap_or(u64::MAX)) - b.start_frame;
                let pos = b.offset_secs + played as f64 / self.shared.out_rate as f64;
                let dur = b.duration_secs.or(self.tracks[b.track].info.duration_secs);
                (Some(b.track), dur.map_or(pos, |d| pos.min(d)), dur)
            }
            None => (None, 0.0, None),
        };
        Snapshot {
            status,
            track,
            position_secs,
            duration_secs,
            volume: self.volume,
            shuffle: st.shuffle,
            repeat: st.repeat,
            order: st.order.clone(),
            last_error: st.last_error.clone(),
        }
    }

    /// The latest music analysis of what is being heard (silence while
    /// paused or stopped).
    pub fn audio_frame(&self) -> AudioFrame {
        self.analysis.frame()
    }

    /// Replaces the whole list and queue, starting at `start` from `secs`
    /// (e.g. playing an album, or restoring a saved queue).
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize, secs: f64) {
        self.generation += 1;
        self.tracks = tracks.clone();
        self.shared.paused.store(false, Ordering::Relaxed);
        self.send(Command::Replace {
            tracks,
            start,
            secs,
            generation: self.generation,
        });
    }

    /// Adds a track to the end of the queue. Starts playing if nothing was.
    pub fn append(&mut self, track: Track) {
        self.tracks.push(track.clone());
        self.send(Command::Append(track));
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn toggle_pause(&mut self) {
        let snap = self.snapshot();
        match (snap.status, snap.track) {
            (Status::Stopped, Some(t)) => self.jump_to(t),
            (Status::Stopped, None) => {
                if let Some(&first) = snap.order.first() {
                    self.jump_to(first);
                }
            }
            _ => {
                self.shared.paused.fetch_xor(true, Ordering::Relaxed);
            }
        }
    }

    pub fn jump_to(&mut self, track: usize) {
        self.shared.paused.store(false, Ordering::Relaxed);
        self.send(Command::JumpTo(track));
    }

    pub fn next(&mut self) {
        if let Some(from) = self.snapshot().track {
            self.send(Command::Next { from });
        }
    }

    pub fn prev(&mut self) {
        let snap = self.snapshot();
        if let Some(from) = snap.track {
            self.send(Command::Prev {
                from,
                position: snap.position_secs,
            });
        }
    }

    /// Relative seek in seconds, clamped to the track.
    pub fn seek_by(&mut self, delta: f64) {
        let snap = self.snapshot();
        if let (Some(track), Status::Playing | Status::Paused) = (snap.track, snap.status) {
            let mut target = (snap.position_secs + delta).max(0.0);
            if let Some(d) = snap.duration_secs {
                target = target.min((d - 0.5).max(0.0));
            }
            self.send(Command::Seek {
                track,
                secs: target,
            });
        }
    }

    /// Volume in percent, 0..=100.
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume.min(100);
        self.shared
            .gain
            .store(volume_to_gain(self.volume).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.send(Command::SetShuffle(on));
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.send(Command::SetRepeat(repeat));
    }

    pub fn toggle_shuffle(&mut self) {
        let on = !self.snapshot().shuffle;
        self.send(Command::SetShuffle(on));
    }

    pub fn cycle_repeat(&mut self) {
        let next = self.snapshot().repeat.cycle();
        self.send(Command::SetRepeat(next));
    }

    fn send(&self, cmd: Command) {
        // Only fails if the decoder thread died; the snapshot's error says why.
        let _ = self.commands.send(cmd);
    }
}

/// Perceptual volume curve: equal steps sound roughly equal.
fn volume_to_gain(volume: u8) -> f32 {
    let v = volume.min(100) as f32 / 100.0;
    v * v
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut consumer: Consumer<f32>,
    mut tap: Producer<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut gain_now = f32::from_bits(shared.gain.load(Ordering::Relaxed));
    let err_shared = shared.clone();

    // Real-time callback: no allocation, no blocking locks.
    let data_fn = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        if shared.flush.load(Ordering::Acquire) {
            // The decoder pushes nothing while a flush is pending, so this
            // count is only old audio. Claim the request before draining:
            // if the decoder already gave up waiting and cancelled it, the
            // ring may hold new audio, which must not be thrown away. The
            // decoder owns the consumed counter across a flush.
            let n = consumer.slots();
            if shared
                .flush
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                && let Ok(chunk) = consumer.read_chunk(n)
            {
                chunk.commit_all();
            }
        }

        let frames = data.len() / channels.max(1);
        if shared.paused.load(Ordering::Relaxed) {
            data.fill(T::from_sample(0.0f32));
            // Keep the analyser's clock running so its levels fall to zero.
            for _ in 0..frames {
                if tap.push(0.0).is_err() {
                    break;
                }
            }
            return;
        }

        let available = (consumer.slots() / OUT_CHANNELS).min(frames);
        let gain_target = f32::from_bits(shared.gain.load(Ordering::Relaxed));
        let step = (gain_target - gain_now) / frames.max(1) as f32;

        let chunk = consumer.read_chunk(available * OUT_CHANNELS).ok();
        let (a, b) = chunk.as_ref().map_or((&[][..], &[][..]), |c| c.as_slices());
        let mut samples = a.iter().chain(b.iter()).copied();

        for frame in data.chunks_mut(channels.max(1)) {
            gain_now += step;
            // Past `available` this is an underrun: play silence.
            let l = samples.next().unwrap_or(0.0);
            let r = samples.next().unwrap_or(0.0);
            // The analyser hears the music before the volume control, so
            // visuals don't shrink when you turn it down. A full tap means
            // the analyser is behind: drop the sample, never wait.
            let _ = tap.push((l + r) * 0.5);
            let (l, r) = (l * gain_now, r * gain_now);
            match frame.len() {
                1 => frame[0] = T::from_sample((l + r) * 0.5),
                _ => {
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                    for s in &mut frame[2..] {
                        *s = T::from_sample(0.0f32);
                    }
                }
            }
        }
        gain_now = gain_target;
        if let Some(c) = chunk {
            c.commit_all();
        }
        shared
            .consumed
            .fetch_add(available as u64, Ordering::AcqRel);
    };

    let err_fn = move |err: cpal::Error| {
        if let Ok(mut st) = err_shared.state.try_lock() {
            st.last_error = Some(format!("audio device: {err}"));
        }
    };

    device
        .build_output_stream(*config, data_fn, err_fn, None)
        .map_err(|e| AudioError::Device(e.to_string()))
}

/// Owns the queue and the open file; decodes ahead into the ring buffer.
struct Decoder {
    tracks: Vec<Track>,
    /// Seed for new queues' shuffle.
    seed: u64,
    shared: Arc<Shared>,
    producer: Producer<f32>,
    commands: Receiver<Command>,
    queue: Option<Queue>,
    reader: Option<(usize, TrackReader)>,
    /// Decoded samples not yet in the ring buffer.
    pending: Vec<f32>,
    /// Output frames written to the ring buffer since start.
    pushed: u64,
}

impl Decoder {
    fn new(
        tracks: Vec<Track>,
        shared: Arc<Shared>,
        producer: Producer<f32>,
        commands: Receiver<Command>,
        queue: Option<Queue>,
    ) -> Self {
        Decoder {
            tracks,
            seed: 1,
            shared,
            producer,
            commands,
            queue,
            reader: None,
            pending: Vec::new(),
            pushed: 0,
        }
    }

    fn run(&mut self) {
        self.publish_queue();
        if let Some(t) = self.queue.as_ref().map(Queue::current) {
            self.load(t, 0.0);
        }
        loop {
            let busy = self.reader.is_some() || !self.pending.is_empty();
            let ring_full = !self.pending.is_empty() && self.producer.slots() < OUT_CHANNELS * 256;
            let cmd = if busy && !ring_full {
                match self.commands.try_recv() {
                    Ok(c) => Some(c),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else {
                match self.commands.recv_timeout(Duration::from_millis(10)) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            if let Some(cmd) = cmd {
                self.handle(cmd);
                continue;
            }
            self.fill();
        }
    }

    fn handle(&mut self, cmd: Command) {
        let cmd = match cmd {
            Command::Replace {
                tracks,
                start,
                secs,
                generation,
            } => {
                let (shuffle, repeat) = self
                    .queue
                    .as_ref()
                    .map_or((false, Repeat::Off), |q| (q.shuffle(), q.repeat()));
                self.flush();
                self.reader = None;
                self.tracks = tracks;
                self.queue = Queue::new(self.tracks.len(), start, false, self.seed);
                if let Some(q) = self.queue.as_mut() {
                    q.set_repeat(repeat);
                    q.set_shuffle(shuffle);
                }
                self.lock_state().generation = generation;
                match self.queue.as_ref().map(Queue::current) {
                    Some(t) => self.load(t, secs),
                    None => self.stop(),
                }
                self.publish_queue();
                return;
            }
            Command::Append(track) => {
                self.tracks.push(track);
                match self.queue.as_mut() {
                    Some(q) => {
                        q.push();
                        if self.reader.is_none() && self.pending.is_empty() {
                            let heard_end = {
                                let consumed = self.shared.consumed.load(Ordering::Acquire);
                                self.lock_state().ended_at.is_some_and(|e| consumed >= e)
                            };
                            if heard_end {
                                // Playback had stopped: start the new track.
                                let t = self.tracks.len() - 1;
                                if let Some(q) = self.queue.as_mut() {
                                    q.jump_to(t);
                                }
                                self.load(t, 0.0);
                            } else {
                                // The last track is still sounding: follow it
                                // gaplessly.
                                self.advance();
                            }
                        }
                    }
                    None => {
                        self.queue = Queue::new(1, 0, false, self.seed);
                        self.load(0, 0.0);
                    }
                }
                self.publish_queue();
                return;
            }
            other => other,
        };
        let Some(queue) = self.queue.as_mut() else {
            return;
        };
        match cmd {
            Command::JumpTo(t) => {
                queue.jump_to(t);
                self.load(t, 0.0);
            }
            Command::Next { from } => {
                queue.jump_to(from);
                match queue.skip_forward() {
                    Some(t) => self.load(t, 0.0),
                    None => self.stop(),
                }
            }
            Command::Prev { from, position } => {
                queue.jump_to(from);
                let t = if position > PREV_RESTART_SECS {
                    from
                } else {
                    queue.skip_back()
                };
                self.load(t, 0.0);
            }
            Command::Seek { track, secs } => {
                queue.jump_to(track);
                self.load(track, secs);
            }
            Command::SetShuffle(on) => queue.set_shuffle(on),
            Command::SetRepeat(r) => queue.set_repeat(r),
            Command::Replace { .. } | Command::Append(_) => unreachable!("handled above"),
        }
        self.publish_queue();
    }

    fn publish_queue(&self) {
        let mut st = self.lock_state();
        match &self.queue {
            Some(q) => {
                st.order = q.order().0.to_vec();
                st.shuffle = q.shuffle();
                st.repeat = q.repeat();
            }
            None => st.order.clear(),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, DecoderState> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Discards everything buffered so the next sample heard is new.
    fn flush(&mut self) {
        self.pending.clear();
        self.shared.flush.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.shared.flush.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        // The callback never ran (device starting up or stalled): cancel the
        // request so it cannot later drain audio pushed after this point.
        self.shared.flush.store(false, Ordering::Release);
        // If the device stalled, the callback never drained; either way
        // nothing old will be counted as the new track's position.
        self.shared.consumed.store(self.pushed, Ordering::Release);
        let mut st = self.lock_state();
        st.boundaries.clear();
        st.ended_at = None;
    }

    fn stop(&mut self) {
        self.flush();
        self.reader = None;
        self.lock_state().ended_at = Some(self.pushed);
    }

    /// Interrupting load: flush, then play `track` from `secs`.
    fn load(&mut self, track: usize, secs: f64) {
        self.flush();
        // Reuse the open file when seeking within the same track.
        let reuse = matches!(&self.reader, Some((t, _)) if *t == track);
        if !reuse {
            self.reader = None;
        }
        let opened = if reuse { Ok(()) } else { self.open(track) };
        let mut offset = 0.0;
        if opened.is_ok()
            && secs > 0.0
            && let Some((_, r)) = self.reader.as_mut()
        {
            match r.seek(secs) {
                Ok(actual) => offset = actual,
                Err(e) => self.lock_state().last_error = Some(e.to_string()),
            }
        }
        match opened {
            Ok(()) => self.mark_boundary(track, offset),
            Err(()) => self.advance(),
        }
    }

    fn open(&mut self, track: usize) -> Result<(), ()> {
        let path = &self.tracks[track].path;
        match TrackReader::open(path, self.shared.out_rate) {
            Ok(r) => {
                self.reader = Some((track, r));
                Ok(())
            }
            Err(e) => {
                self.lock_state().last_error = Some(e.to_string());
                self.reader = None;
                Err(())
            }
        }
    }

    fn mark_boundary(&mut self, track: usize, offset_secs: f64) {
        let duration_secs = self.reader.as_ref().and_then(|(_, r)| r.duration_secs());
        let start_frame = self.pushed + (self.pending.len() / OUT_CHANNELS) as u64;
        let mut st = self.lock_state();
        st.ended_at = None;
        st.boundaries.push_back(Boundary {
            start_frame,
            track,
            offset_secs,
            duration_secs,
        });
    }

    /// Current track finished: continue gaplessly with the next one,
    /// skipping files that fail to open.
    fn advance(&mut self) {
        let attempts = self.queue.as_ref().map_or(0, Queue::len);
        for _ in 0..attempts {
            let Some(next) = self.queue.as_mut().and_then(Queue::advance_auto) else {
                break;
            };
            if self.open(next).is_ok() {
                self.mark_boundary(next, 0.0);
                self.publish_queue();
                return;
            }
        }
        self.reader = None;
        let end = self.pushed + (self.pending.len() / OUT_CHANNELS) as u64;
        self.lock_state().ended_at = Some(end);
        self.publish_queue();
    }

    /// Moves decoded audio into the ring buffer, decoding more when needed.
    fn fill(&mut self) {
        if !self.pending.is_empty() {
            let n = self.producer.slots().min(self.pending.len());
            let n = n - n % OUT_CHANNELS;
            if n > 0
                && let Ok(chunk) = self.producer.write_chunk_uninit(n)
            {
                chunk.fill_from_iter(self.pending.drain(..n));
                self.pushed += (n / OUT_CHANNELS) as u64;
            }
            return;
        }
        let Some((_, reader)) = self.reader.as_mut() else {
            return;
        };
        match reader.read_into(&mut self.pending) {
            Ok(true) => {}
            Ok(false) => self.advance(),
            Err(e) => {
                self.lock_state().last_error = Some(e.to_string());
                self.advance();
            }
        }
    }
}
