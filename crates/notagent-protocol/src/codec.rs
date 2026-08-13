//! Port von `packages/protocol/src/codec.ts`.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cbor::{CborOptions, CborValue, decode_cbor, encode_cbor};
use crate::framing::{
    DEFAULT_MAX_FRAME_LENGTH, FrameDecoder, FrameDecoderOptions, FrameError, assert_complete_frame,
    encode_frame,
};
use crate::schemas::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ProtocolValidationError {
    message: String,
}

impl ProtocolValidationError {
    /// Der zweite TS-Parameter (`_value`) wird bewusst nicht gespeichert:
    /// Fehler dürfen abgelehnte Payloads nicht festhalten.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Port von `isProtocolValue` + `Check(Schema, value)`.
///
/// `isProtocolValue` lässt ausschließlich JSON-Werte zu. Byte-Strings sind
/// keine Protokollwerte; Zyklen, `undefined` und Nicht-Plain-Objekte sind in
/// `CborValue` nicht darstellbar.
fn parse_protocol_message<T: DeserializeOwned>(
    value: &CborValue,
    kind: &str,
) -> Result<T, ProtocolValidationError> {
    let json = value
        .to_json_value()
        .ok_or_else(|| ProtocolValidationError::new(format!("Invalid {kind} protocol message")))?;
    serde_json::from_value(json)
        .map_err(|_| ProtocolValidationError::new(format!("Invalid {kind} protocol message")))
}

pub fn parse_client_message(value: &CborValue) -> Result<ClientMessage, ProtocolValidationError> {
    parse_protocol_message(value, "client")
}

pub fn parse_server_message(value: &CborValue) -> Result<ServerMessage, ProtocolValidationError> {
    parse_protocol_message(value, "server")
}

fn bounded_error_message(message: &str) -> String {
    if message.chars().count() <= 500 {
        return message.to_owned();
    }
    let prefix: String = message.chars().take(497).collect();
    format!("{prefix}...")
}

fn to_cbor_value<T: Serialize>(
    value: &T,
    kind: &str,
) -> Result<CborValue, ProtocolValidationError> {
    let json = serde_json::to_value(value).map_err(|error| {
        ProtocolValidationError::new(format!(
            "Unable to encode {kind} protocol message: {}",
            bounded_error_message(&error.to_string())
        ))
    })?;
    Ok(CborValue::from_json_value(&json))
}

fn encode_protocol_message<T, P>(
    value: &T,
    parse: P,
    kind: &str,
    options: Option<FrameDecoderOptions>,
) -> Result<Vec<u8>, ProtocolValidationError>
where
    T: Serialize,
    P: Fn(&CborValue) -> Result<T, ProtocolValidationError>,
{
    let candidate = to_cbor_value(value, kind)?;
    let validated = parse(&candidate)?;
    let validated = to_cbor_value(&validated, kind)?;
    let max_frame_length = options
        .and_then(|options| options.max_frame_length)
        .unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    let encode = || -> Result<Vec<u8>, String> {
        let payload = encode_cbor(
            &validated,
            Some(CborOptions::with_max_byte_length(max_frame_length)),
        )
        .map_err(|error| error.message().to_owned())?;
        let frame = encode_frame(&payload).map_err(|error| error.message().to_owned())?;
        assert_complete_frame(
            &frame,
            Some(FrameDecoderOptions::with_max_frame_length(max_frame_length)),
        )
        .map_err(|error| error.message().to_owned())?;
        Ok(frame)
    };
    encode().map_err(|message| {
        ProtocolValidationError::new(format!(
            "Unable to encode {kind} protocol message: {}",
            bounded_error_message(&message)
        ))
    })
}

/// Validates and encodes one complete length-prefixed client message.
pub fn encode_client_message(
    message: &ClientMessage,
    options: Option<FrameDecoderOptions>,
) -> Result<Vec<u8>, ProtocolValidationError> {
    encode_protocol_message(message, parse_client_message, "client", options)
}

/// Validates and encodes one complete length-prefixed server message.
pub fn encode_server_message(
    message: &ServerMessage,
    options: Option<FrameDecoderOptions>,
) -> Result<Vec<u8>, ProtocolValidationError> {
    encode_protocol_message(message, parse_server_message, "server", options)
}

struct ValidatedMessageDecoder<T> {
    failed: bool,
    frames: FrameDecoder,
    kind: &'static str,
    max_frame_length: u64,
    parse: fn(&CborValue) -> Result<T, ProtocolValidationError>,
}

impl<T> ValidatedMessageDecoder<T> {
    fn new(
        kind: &'static str,
        parse: fn(&CborValue) -> Result<T, ProtocolValidationError>,
        options: Option<FrameDecoderOptions>,
    ) -> Result<Self, FrameError> {
        Ok(Self {
            failed: false,
            frames: FrameDecoder::new(options)?,
            kind,
            max_frame_length: options
                .and_then(|options| options.max_frame_length)
                .unwrap_or(DEFAULT_MAX_FRAME_LENGTH),
            parse,
        })
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<T>, ProtocolValidationError> {
        if self.failed {
            return Err(ProtocolValidationError::new(format!(
                "{} message decoder has failed",
                self.kind
            )));
        }
        let mut messages = Vec::new();
        let result = (|| -> Result<(), ProtocolValidationError> {
            let frames = self
                .frames
                .push(chunk)
                .map_err(|error| self.wrap_frame_error("frame", error.message()))?;
            for frame in frames {
                let value = decode_cbor(
                    &frame,
                    Some(CborOptions::with_max_byte_length(self.max_frame_length)),
                )
                .map_err(|error| self.wrap_frame_error("frame", error.message()))?;
                messages.push((self.parse)(&value)?);
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(messages),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    fn wrap_frame_error(&self, what: &str, message: &str) -> ProtocolValidationError {
        ProtocolValidationError::new(format!(
            "Invalid {} protocol {what}: {}",
            self.kind,
            bounded_error_message(message)
        ))
    }

    fn end(&mut self) -> Result<(), ProtocolValidationError> {
        if self.failed {
            return Err(ProtocolValidationError::new(format!(
                "{} message decoder has failed",
                self.kind
            )));
        }
        match self.frames.end() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failed = true;
                Err(self.wrap_frame_error("framing", error.message()))
            }
        }
    }
}

/// Incrementally decodes and validates framed client messages.
pub struct ClientMessageDecoder {
    decoder: ValidatedMessageDecoder<ClientMessage>,
}

impl ClientMessageDecoder {
    pub fn new(options: Option<FrameDecoderOptions>) -> Result<Self, FrameError> {
        Ok(Self {
            decoder: ValidatedMessageDecoder::new("client", parse_client_message, options)?,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ClientMessage>, ProtocolValidationError> {
        self.decoder.push(chunk)
    }

    pub fn end(&mut self) -> Result<(), ProtocolValidationError> {
        self.decoder.end()
    }
}

/// Incrementally decodes and validates framed server messages.
pub struct ServerMessageDecoder {
    decoder: ValidatedMessageDecoder<ServerMessage>,
}

impl ServerMessageDecoder {
    pub fn new(options: Option<FrameDecoderOptions>) -> Result<Self, FrameError> {
        Ok(Self {
            decoder: ValidatedMessageDecoder::new("server", parse_server_message, options)?,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<ServerMessage>, ProtocolValidationError> {
        self.decoder.push(chunk)
    }

    pub fn end(&mut self) -> Result<(), ProtocolValidationError> {
        self.decoder.end()
    }
}

pub fn create_client_message_decoder(
    options: Option<FrameDecoderOptions>,
) -> Result<ClientMessageDecoder, FrameError> {
    ClientMessageDecoder::new(options)
}

pub fn create_server_message_decoder(
    options: Option<FrameDecoderOptions>,
) -> Result<ServerMessageDecoder, FrameError> {
    ServerMessageDecoder::new(options)
}

/// TS prüft zusätzlich `Number.isInteger(version)`; in Rust ist `version`
/// bereits ganzzahlig (Abweichung Klasse 1).
pub fn is_supported_protocol_version(version: u64) -> bool {
    version == PROTOCOL_VERSION
}
