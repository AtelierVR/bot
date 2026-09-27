use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Parse an Ogg Opus file and extract individual Opus packets.
/// Returns packets where each packet is a valid Opus frame ready to send.
pub fn parse_ogg_opus(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(data));
    let mut packets = Vec::new();

    while let Ok(Some(packet)) = reader.read_packet() {
        // Each Ogg packet for Opus contains one complete Opus frame
        if packet.data.is_empty() {
            continue;
        }
        packets.push(packet.data.to_vec());
    }

    if packets.is_empty() {
        return Err("No Opus packets found in Ogg file".into());
    }

    Ok(packets)
}

/// Maximum number of samples per channel in a 120 ms Opus frame at 48 kHz.
const MAX_FRAME_SAMPLES: usize = 48_000 * 120 / 1000;

/// Samples per mono frame at 48 kHz for the 20 ms frame the pipeline sends
/// (matches `OpusConfig.SamplesPerFrame` on the Unity side).
const FRAME_SAMPLES: usize = 960;

/// Playout latency target: a frame is only emitted once it had this long to arrive.
/// Unity sends from its render loop (0/1/2-frame bursts) and the relay adds jitter, so
/// playing frames as they arrive starves the output as soon as one is late.
const TARGET_LATENCY: Duration = Duration::from_millis(60);

/// Granularity of the playout loop.
const TICK_INTERVAL: Duration = Duration::from_millis(5);

/// How long a speaker is considered idle before the next frame restarts the
/// timeline (the sender does not stream while it has nothing to say).
const IDLE_RESET: Duration = Duration::from_millis(400);

/// At most this many missing frames in a row are concealed with Opus PLC.
const MAX_CONCEALED_FRAMES: i32 = 3;

/// A hole is only treated as packet loss while the stream is still fresh; after
/// that it comes from the sender's voice-activity gate, so it must be skipped
/// instead of concealed to keep the playout in sync with real time.
const CONCEAL_MAX_SILENCE: Duration = Duration::from_millis(150);

/// Decoded PCM chunks queued for the audio thread (20 ms each => 500 ms slack).
const PCM_QUEUE: usize = 25;

/// A `rodio` source that plays decoded PCM chunks arriving over a channel.
///
/// It must never block: it is pulled by the device thread, so waiting for the next
/// chunk stalls the output (audible crackle). It yields silence when the queue is
/// empty while the playout thread feeds it at a steady 20 ms cadence.
struct PcmSource {
    rx: mpsc::Receiver<Vec<f32>>,
    current: Vec<f32>,
    pos: usize,
}

impl PcmSource {
    fn new(rx: mpsc::Receiver<Vec<f32>>) -> Self {
        Self {
            rx,
            current: Vec::new(),
            pos: 0,
        }
    }
}

impl Iterator for PcmSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if self.pos < self.current.len() {
                let sample = self.current[self.pos];
                self.pos += 1;
                return Some(sample);
            }
            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.pos = 0;
                }
                // Underrun: output silence instead of blocking the audio thread.
                Err(mpsc::TryRecvError::Empty) => return Some(0.0),
                // Disconnected: the program is shutting down.
                Err(mpsc::TryRecvError::Disconnected) => return None,
            }
        }
    }
}

impl rodio::Source for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        1
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

/// One speaker's playout state: jitter buffer, decoder and playout head.
struct StreamState {
    decoder: opus::Decoder,
    /// Next frame index to emit (meaningful once `primed`).
    next_index: i32,
    /// Frames received but not emitted yet, ordered by the sender's frame index.
    pending: BTreeMap<i32, Vec<u8>>,
    /// When the buffer first received data since it was idle (jitter priming).
    priming_since: Option<Instant>,
    /// When the last frame arrived.
    last_arrival: Option<Instant>,
    /// Whether the playout head is locked on the sender's frame indices.
    primed: bool,
}

impl StreamState {
    fn new() -> Self {
        Self {
            decoder: opus::Decoder::new(48_000, opus::Channels::Mono)
                .expect("failed to create Opus decoder"),
            next_index: 0,
            pending: BTreeMap::new(),
            priming_since: None,
            last_arrival: None,
            primed: false,
        }
    }

    /// Decode a frame into PCM. An empty payload is the sender's keep-alive for a
    /// closed voice gate (silence), not loss: loss is a *missing* frame index and is
    /// handled by `conceal()`.
    fn decode(&mut self, payload: &[u8]) -> Vec<f32> {
        if payload.is_empty() {
            // Gated silence: emit silence without disturbing the decoder state.
            return vec![0f32; FRAME_SAMPLES];
        }

        let mut output = vec![0f32; MAX_FRAME_SAMPLES];
        match self.decoder.decode_float(payload, &mut output, false) {
            Ok(n) if n > 0 => {
                output.truncate(n);
                output
            }
            // A failed decode keeps the playout aligned on silence.
            _ => vec![0f32; FRAME_SAMPLES],
        }
    }

    /// Asks Opus for packet loss concealment (a plausible 20 ms continuation).
    fn conceal(&mut self) -> Vec<f32> {
        let mut output = vec![0f32; MAX_FRAME_SAMPLES];
        match self.decoder.decode_float(&[], &mut output, false) {
            Ok(n) if n > 0 => {
                output.truncate(n);
                output
            }
            _ => vec![0f32; FRAME_SAMPLES],
        }
    }

    /// Emits every frame that is playable by now for this speaker.
    fn emit(&mut self, pcm_tx: &mpsc::SyncSender<Vec<f32>>) {
        let now = Instant::now();

        // Idle speaker: forget the timeline so the next burst re-primes cleanly
        // instead of concealing a hole that is really just silence.
        if self.pending.is_empty()
            && self
                .last_arrival
                .map_or(false, |last| now.duration_since(last) > IDLE_RESET)
        {
            self.primed = false;
            self.priming_since = None;
            return;
        }

        if self.pending.is_empty() {
            return;
        }

        // Jitter priming: wait until the buffer holds TARGET_LATENCY worth of
        // frames before locking the playout head, so a late arrival cannot starve it.
        if !self.primed {
            let waited = self
                .priming_since
                .map_or(Duration::ZERO, |since| now.duration_since(since));
            if waited < TARGET_LATENCY {
                return;
            }

            self.next_index = *self.pending.keys().next().expect("pending is not empty");
            self.primed = true;
        }

        loop {
            // In-order frame: decode and hand it to the audio thread.
            if let Some(payload) = self.pending.remove(&self.next_index) {
                let pcm = self.decode(&payload);
                let _ = pcm_tx.try_send(pcm);
                self.next_index += 1;
                continue;
            }

            let Some(&first) = self.pending.keys().next() else {
                break;
            };

            if first > self.next_index {
                let gap = first - self.next_index;
                let fresh = self
                    .last_arrival
                    .map_or(false, |last| now.duration_since(last) <= CONCEAL_MAX_SILENCE);

                if gap <= MAX_CONCEALED_FRAMES && fresh {
                    // Lost packets in the middle of an active stream: let Opus
                    // conceal them instead of cutting a hole in the audio.
                    let pcm = self.conceal();
                    let _ = pcm_tx.try_send(pcm);
                    self.next_index += 1;
                    continue;
                }

                // Intentional silence (the sender stops streaming through its
                // gate) or a long outage: jump to the next available frame.
                self.next_index = first;
                continue;
            }

            // Late frame behind the playout head: unusable, drop it.
            self.pending.pop_first();
        }
    }
}

/// Shared voice playback state: buffers incoming Opus frames per speaker, decodes
/// them in order (with concealment) and forwards PCM to the audio output thread.
pub struct VoicePlayback {
    /// Kept so the PCM channel outlives the playout thread handle.
    _pcm_tx: mpsc::SyncSender<Vec<f32>>,
    streams: Arc<Mutex<HashMap<u16, StreamState>>>,
}

impl VoicePlayback {
    /// Start the output and playout threads and return a shared handle.
    pub fn start() -> Result<Arc<Self>, String> {
        let (pcm_tx, pcm_rx) = mpsc::sync_channel::<Vec<f32>>(PCM_QUEUE);

        std::thread::Builder::new()
            .name("voice-playback".to_string())
            .spawn(move || {
                let (stream, handle) = match rodio::OutputStream::try_default() {
                    Ok(pair) => pair,
                    Err(e) => {
                        eprintln!("[voice] failed to open audio output: {e}");
                        return;
                    }
                };
                let sink = match rodio::Sink::try_new(&handle) {
                    Ok(sink) => sink,
                    Err(e) => {
                        eprintln!("[voice] failed to create audio sink: {e}");
                        return;
                    }
                };
                sink.append(PcmSource::new(pcm_rx));

                // Keep the output stream alive until the process exits.
                let _keep_alive = (stream, sink);
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            })
            .map_err(|e| format!("failed to spawn audio thread: {e}"))?;

        let streams: Arc<Mutex<HashMap<u16, StreamState>>> = Arc::new(Mutex::new(HashMap::new()));

        let playout_streams = Arc::clone(&streams);
        let playout_tx = pcm_tx.clone();
        std::thread::Builder::new()
            .name("voice-playout".to_string())
            .spawn(move || loop {
                std::thread::sleep(TICK_INTERVAL);
                match playout_streams.lock() {
                    Ok(mut streams) => {
                        for state in streams.values_mut() {
                            state.emit(&playout_tx);
                        }
                    }
                    // A poisoned lock only means one frame was skipped.
                    Err(_) => continue,
                }
            })
            .map_err(|e| format!("failed to spawn playout thread: {e}"))?;

        Ok(Arc::new(Self { _pcm_tx: pcm_tx, streams }))
    }

    /// Buffer a single voice frame for `player_id`.
    ///
    /// Frames are ordered by the sender's monotonic frame index and decoded by the
    /// playout thread once the jitter buffer had time to fill. Decoding on arrival (the
    /// previous behaviour) plays whatever arrived as fast as it arrived, which turns the
    /// bursty sender and any network reordering into crackle.
    /// An empty payload is a valid frame (the sender's silence keep-alive) and the same
    /// frame index twice (broadcast to several listeners) is ignored.
    pub fn play_frame(&self, player_id: u16, frame_index: i32, sample: &[u8]) {
        let mut streams = match self.streams.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        let now = Instant::now();
        let state = streams.entry(player_id).or_insert_with(StreamState::new);

        state.last_arrival = Some(now);
        state.priming_since.get_or_insert(now);

        state
            .pending
            .entry(frame_index)
            .or_insert_with(|| sample.to_vec());
    }
}
