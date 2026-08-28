use super::options::{
    CborError, CborOptions, MAX_UINT32, ResolvedCborOptions, UINT32_BASE, resolve_options,
};
use super::value::{CborValue, is_integer, is_negative_zero, is_safe_integer};

struct CborWriter {
    buffer: Vec<u8>,
    max_byte_length: u64,
}

impl CborWriter {
    fn new(max_byte_length: u64) -> Self {
        Self {
            buffer: Vec::with_capacity(max_byte_length.min(256) as usize),
            max_byte_length,
        }
    }

    fn write_byte(&mut self, value: u8) -> Result<(), CborError> {
        self.ensure_capacity(1)?;
        self.buffer.push(value);
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), CborError> {
        self.ensure_capacity(bytes.len() as u64)?;
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn write_uint16(&mut self, value: u16) -> Result<(), CborError> {
        self.ensure_capacity(2)?;
        self.buffer.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn write_uint32(&mut self, value: u32) -> Result<(), CborError> {
        self.ensure_capacity(4)?;
        self.buffer.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn write_uint64(&mut self, value: u64) -> Result<(), CborError> {
        let high = value / UINT32_BASE;
        let low = value - high * UINT32_BASE;
        self.write_uint32(high as u32)?;
        self.write_uint32(low as u32)
    }

    fn write_float64(&mut self, value: f64) -> Result<(), CborError> {
        self.ensure_capacity(9)?;
        self.buffer.push(0xfb);
        self.buffer.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.buffer
    }

    fn ensure_capacity(&mut self, additional_bytes: u64) -> Result<(), CborError> {
        let required = self.buffer.len() as u64 + additional_bytes;
        if required > self.max_byte_length {
            return Err(CborError::cbor(format!(
                "CBOR byte length exceeds configured limit of {}",
                self.max_byte_length
            )));
        }
        Ok(())
    }
}

fn write_argument(writer: &mut CborWriter, major_type: u8, value: u64) -> Result<(), CborError> {
    let prefix = major_type << 5;
    if value < 24 {
        writer.write_byte(prefix | value as u8)
    } else if value <= 0xff {
        writer.write_byte(prefix | 24)?;
        writer.write_byte(value as u8)
    } else if value <= 0xffff {
        writer.write_byte(prefix | 25)?;
        writer.write_uint16(value as u16)
    } else if value <= MAX_UINT32 {
        writer.write_byte(prefix | 26)?;
        writer.write_uint32(value as u32)
    } else {
        writer.write_byte(prefix | 27)?;
        writer.write_uint64(value)
    }
}

fn encode_text(
    writer: &mut CborWriter,
    value: &str,
    options: &ResolvedCborOptions,
) -> Result<(), CborError> {
    let bytes = value.as_bytes();
    if bytes.len() as u64 > options.max_byte_length {
        return Err(CborError::cbor(format!(
            "CBOR text string length exceeds configured limit of {}",
            options.max_byte_length
        )));
    }
    // The `textDecoder.decode(bytes) !== value` check (lossy strings with lone
    // surrogates) is dropped: a Rust `String` is valid UTF-8 by construction.
    write_argument(writer, 3, bytes.len() as u64)?;
    writer.write_bytes(bytes)
}

fn encode_value(
    writer: &mut CborWriter,
    value: &CborValue,
    options: &ResolvedCborOptions,
    depth: u64,
) -> Result<(), CborError> {
    if depth > options.max_depth {
        return Err(CborError::cbor(format!(
            "CBOR nesting depth exceeds configured limit of {}",
            options.max_depth
        )));
    }

    match value {
        CborValue::Null => writer.write_byte(0xf6),
        CborValue::Bool(value) => writer.write_byte(if *value { 0xf5 } else { 0xf4 }),
        CborValue::Number(value) => {
            let value = *value;
            if !value.is_finite() {
                return Err(CborError::cbor("CBOR numbers must be finite"));
            }
            if is_integer(value) && !is_negative_zero(value) {
                if !is_safe_integer(value) {
                    return Err(CborError::cbor(
                        "CBOR integers must be safe JavaScript integers",
                    ));
                }
                if value >= 0.0 {
                    write_argument(writer, 0, value as u64)
                } else {
                    write_argument(writer, 1, (-1.0 - value) as u64)
                }
            } else {
                writer.write_float64(value)
            }
        }
        CborValue::Text(value) => encode_text(writer, value, options),
        CborValue::Bytes(value) => {
            if value.len() as u64 > options.max_byte_length {
                return Err(CborError::cbor(format!(
                    "CBOR byte string length exceeds configured limit of {}",
                    options.max_byte_length
                )));
            }
            write_argument(writer, 2, value.len() as u64)?;
            writer.write_bytes(value)
        }
        CborValue::Array(items) => {
            if items.len() as u64 > options.max_container_length {
                return Err(CborError::cbor(format!(
                    "CBOR array length exceeds configured limit of {}",
                    options.max_container_length
                )));
            }
            write_argument(writer, 4, items.len() as u64)?;
            for item in items {
                encode_value(writer, item, options, depth + 1)?;
            }
            Ok(())
        }
        CborValue::Map(entries) => {
            if entries.len() as u64 > options.max_container_length {
                return Err(CborError::cbor(format!(
                    "CBOR map length exceeds configured limit of {}",
                    options.max_container_length
                )));
            }
            write_argument(writer, 5, entries.len() as u64)?;
            for (key, entry_value) in entries {
                encode_text(writer, key, options)?;
                encode_value(writer, entry_value, options, depth + 1)?;
            }
            Ok(())
        }
    }
}

/// Encodes the protocol's strict, definite-length RFC 8949 subset.
pub fn encode_cbor(value: &CborValue, options: Option<CborOptions>) -> Result<Vec<u8>, CborError> {
    let resolved = resolve_options(options)?;
    let mut writer = CborWriter::new(resolved.max_byte_length);
    encode_value(&mut writer, value, &resolved, 0)?;
    Ok(writer.finish())
}
