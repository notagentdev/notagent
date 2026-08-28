const FRAME_HEADER_LENGTH: usize = 4;
const MAX_UINT32: u64 = 0xffff_ffff;
const PAYLOAD_BLOCK_SIZE: usize = 64 * 1024;

/// Default upper bound for one framed CBOR payload.
pub const DEFAULT_MAX_FRAME_LENGTH: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameDecoderOptions {
    pub max_frame_length: Option<u64>,
}

impl FrameDecoderOptions {
    pub fn with_max_frame_length(max_frame_length: u64) -> Self {
        Self {
            max_frame_length: Some(max_frame_length),
        }
    }
}

/// `resolveMaxFrameLength`. Both are variants of one type here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Corresponds to `class FrameError extends Error`.
    #[error("{0}")]
    Frame(String),
    /// Corresponds to the `RangeError` thrown by `resolveMaxFrameLength`.
    #[error("{0}")]
    Range(String),
}

impl FrameError {
    fn frame(message: impl Into<String>) -> Self {
        Self::Frame(message.into())
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Frame(message) | Self::Range(message) => message,
        }
    }
}

pub(crate) fn resolve_max_frame_length(
    options: Option<FrameDecoderOptions>,
) -> Result<u64, FrameError> {
    let value = options
        .and_then(|options| options.max_frame_length)
        .unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    // Non-integer and negative values cannot be represented in Rust (u64).
    if value > MAX_UINT32 {
        return Err(FrameError::Range(format!(
            "maxFrameLength must be an integer between 0 and {MAX_UINT32}"
        )));
    }
    Ok(value)
}

/// Prefixes a payload with its unsigned 32-bit big-endian byte length.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() as u64 > MAX_UINT32 {
        return Err(FrameError::Range(
            "Frame payload exceeds the unsigned 32-bit length limit".to_owned(),
        ));
    }
    let mut frame = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn read_frame_length(header: &[u8]) -> u64 {
    u64::from(header[0]) * 0x1_000_000
        + u64::from(header[1]) * 0x1_0000
        + u64::from(header[2]) * 0x100
        + u64::from(header[3])
}

/// Validates that bytes contain exactly one complete frame within the configured limit.
pub fn assert_complete_frame(
    frame: &[u8],
    options: Option<FrameDecoderOptions>,
) -> Result<(), FrameError> {
    if frame.len() < FRAME_HEADER_LENGTH {
        return Err(FrameError::frame(
            "Frame does not contain a complete length prefix",
        ));
    }
    let length = read_frame_length(frame);
    let max_frame_length = resolve_max_frame_length(options)?;
    if length > max_frame_length {
        return Err(FrameError::frame(format!(
            "Frame length {length} exceeds configured limit of {max_frame_length}"
        )));
    }
    if frame.len() as u64 != FRAME_HEADER_LENGTH as u64 + length {
        return Err(FrameError::frame(
            "Frame must contain exactly one complete payload",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecoderState {
    Open,
    Ended,
    Failed,
}

/// Incrementally splits arbitrary byte chunks into length-prefixed payloads.
#[derive(Debug)]
pub struct FrameDecoder {
    header: [u8; FRAME_HEADER_LENGTH],
    header_length: usize,
    max_frame_length: u64,
    payload_blocks: Vec<Vec<u8>>,
    payload_length: usize,
    expected_payload_length: Option<usize>,
    state: DecoderState,
}

impl FrameDecoder {
    pub fn new(options: Option<FrameDecoderOptions>) -> Result<Self, FrameError> {
        Ok(Self {
            header: [0; FRAME_HEADER_LENGTH],
            header_length: 0,
            max_frame_length: resolve_max_frame_length(options)?,
            payload_blocks: Vec::new(),
            payload_length: 0,
            expected_payload_length: None,
            state: DecoderState::Open,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        match self.state {
            DecoderState::Ended => return Err(FrameError::frame("Frame decoder has ended")),
            DecoderState::Failed => return Err(FrameError::frame("Frame decoder has failed")),
            DecoderState::Open => {}
        }

        let mut frames: Vec<Vec<u8>> = Vec::new();
        let mut chunk_offset = 0usize;
        while chunk_offset < chunk.len() {
            if self.expected_payload_length.is_none() {
                let header_bytes =
                    (FRAME_HEADER_LENGTH - self.header_length).min(chunk.len() - chunk_offset);
                self.header[self.header_length..self.header_length + header_bytes]
                    .copy_from_slice(&chunk[chunk_offset..chunk_offset + header_bytes]);
                self.header_length += header_bytes;
                chunk_offset += header_bytes;
                if self.header_length < FRAME_HEADER_LENGTH {
                    continue;
                }

                let frame_length = read_frame_length(&self.header);
                self.header_length = 0;
                if frame_length > self.max_frame_length {
                    return Err(self.fail(format!(
                        "Frame length {frame_length} exceeds configured limit of {}",
                        self.max_frame_length
                    )));
                }
                if frame_length == 0 {
                    frames.push(Vec::new());
                    continue;
                }
                self.expected_payload_length = Some(frame_length as usize);
                self.payload_blocks = Vec::new();
                self.payload_length = 0;
            }

            let Some(expected_payload_length) = self.expected_payload_length else {
                continue;
            };
            while chunk_offset < chunk.len() && self.payload_length < expected_payload_length {
                // Block boundaries sit on multiples of PAYLOAD_BLOCK_SIZE: never
                // allocate more memory up front than can already be received.
                let offset_in_block = self.payload_length % PAYLOAD_BLOCK_SIZE;
                if offset_in_block == 0 {
                    let block_size =
                        PAYLOAD_BLOCK_SIZE.min(expected_payload_length - self.payload_length);
                    self.payload_blocks.push(Vec::with_capacity(block_size));
                }
                let block_free = (PAYLOAD_BLOCK_SIZE - offset_in_block)
                    .min(expected_payload_length - self.payload_length);
                let payload_bytes = block_free.min(chunk.len() - chunk_offset);
                let block = self
                    .payload_blocks
                    .last_mut()
                    .expect("a payload block exists");
                block.extend_from_slice(&chunk[chunk_offset..chunk_offset + payload_bytes]);
                self.payload_length += payload_bytes;
                chunk_offset += payload_bytes;
            }
            if self.payload_length == expected_payload_length {
                if self.payload_blocks.len() == 1 {
                    frames.push(self.payload_blocks.remove(0));
                } else {
                    let mut payload = Vec::with_capacity(expected_payload_length);
                    for block in &self.payload_blocks {
                        payload.extend_from_slice(block);
                    }
                    frames.push(payload);
                }
                self.payload_blocks = Vec::new();
                self.expected_payload_length = None;
                self.payload_length = 0;
            }
        }
        Ok(frames)
    }

    pub fn end(&mut self) -> Result<(), FrameError> {
        match self.state {
            DecoderState::Ended => return Err(FrameError::frame("Frame decoder has ended")),
            DecoderState::Failed => return Err(FrameError::frame("Frame decoder has failed")),
            DecoderState::Open => {}
        }
        if self.header_length != 0 || self.expected_payload_length.is_some() {
            return Err(self.fail("Truncated frame at end of stream"));
        }
        self.state = DecoderState::Ended;
        Ok(())
    }

    fn fail(&mut self, message: impl Into<String>) -> FrameError {
        self.state = DecoderState::Failed;
        self.header_length = 0;
        self.payload_blocks = Vec::new();
        self.expected_payload_length = None;
        self.payload_length = 0;
        FrameError::frame(message)
    }
}
