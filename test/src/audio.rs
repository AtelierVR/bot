use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

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

/// A `rodio` source that streams decoded PCM frames arriving over a channel.
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
            // Block until the next decoded frame arrives. `None` on disconnect
            // ends the source (program is shutting down).
            self.current = self.rx.recv().ok()?;
            self.pos = 0;
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

/// Shared voice playback state: decodes Opus frames and forwards PCM to a
/// dedicated audio output thread.
pub struct VoicePlayback {
    pcm_tx: mpsc::Sender<Vec<f32>>,
    decoders: Mutex<HashMap<u16, opus::Decoder>>,
    last_frame: Mutex<HashMap<u16, i32>>,
}

impl VoicePlayback {
    /// Start the audio output thread and return a shared handle.
    pub fn start() -> Result<Arc<Self>, String> {
        let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<f32>>();

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

        Ok(Arc::new(Self {
            pcm_tx,
            decoders: Mutex::new(HashMap::new()),
            last_frame: Mutex::new(HashMap::new()),
        }))
    }

    /// Decode and enqueue a single voice frame.
    ///
    /// Duplicate frames are dropped using the sender's monotonic frame index.
    /// This matters when several bots listen simultaneously: the relay
    /// broadcasts the same frame to every ready bot, so each unique
    /// `(player_id, frame_index)` pair must be played only once.
    pub fn play_frame(&self, player_id: u16, frame_index: i32, sample: &[u8]) {
        if sample.is_empty() {
            return;
        }

        {
            let mut last = match self.last_frame.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if let Some(&seen) = last.get(&player_id) {
                // Reject only exact duplicates. A speaker reconnecting reuses the
                // same player_id but restarts frame_index at 0, so `<=` would drop
                // the entire new stream (silence). `==` keeps the dedup for frames
                // broadcast to multiple listeners while allowing a fresh session.
                if frame_index == seen {
                    return;
                }
            }
            last.insert(player_id, frame_index);
        }

        let pcm = {
            let mut decoders = match self.decoders.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let decoder = decoders.entry(player_id).or_insert_with(|| {
                opus::Decoder::new(48_000, opus::Channels::Mono)
                    .expect("failed to create Opus decoder")
            });
            let mut output = vec![0f32; MAX_FRAME_SAMPLES];
            match decoder.decode_float(sample, &mut output, false) {
                Ok(n) => {
                    output.truncate(n);
                    output
                }
                Err(_) => return,
            }
        };

        let _ = self.pcm_tx.send(pcm);
    }
}
