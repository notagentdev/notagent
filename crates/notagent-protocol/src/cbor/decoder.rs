//! Port of `packages/protocol/src/cbor/decoder.ts`.

use std::collections::HashSet;

use super::options::{CborError, CborOptions, ResolvedCborOptions, UINT32_BASE, resolve_options};
use super::value::{CborValue, is_integer, is_safe_integer};

struct CborReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    options: ResolvedCborOptions,
}

impl<'a> CborReader<'a> {
    fn new(bytes: &'a [u8], options: ResolvedCborOptions) -> Self {
        Self {
            bytes,
            offset: 0,
            options,
        }
    }

    fn decode(&mut self) -> Result<CborValue, CborError> {
        let value = self.read_item(0)?;
        if self.offset != self.bytes.len() {
            return Err(CborError::cbor("CBOR payload contains trailing data"));
        }
        Ok(value)
    }

    fn read_item(&mut self, depth: u64) -> Result<CborValue, CborError> {
        if depth > self.options.max_depth {
            return Err(CborError::cbor(format!(
                "CBOR nesting depth exceeds configured limit of {}",
                self.options.max_depth
            )));
        }
        let initial = self.read_byte()?;
        let major_type = initial >> 5;
        let additional_information = initial & 0x1f;

        match major_type {
            0 => Ok(CborValue::Number(
                self.read_argument(additional_information)? as f64,
            )),
            1 => {
                let value = -1.0 - self.read_argument(additional_information)? as f64;
                if !is_safe_integer(value) {
                    return Err(CborError::cbor(
                        "Decoded CBOR integer is outside the safe range",
                    ));
                }
                Ok(CborValue::Number(value))
            }
            2 => {
                let length = self.read_length(
                    additional_information,
                    "byte string",
                    self.options.max_byte_length,
                )?;
                Ok(CborValue::Bytes(self.read_bytes(length)?.to_vec()))
            }
            3 => {
                let length = self.read_length(
                    additional_information,
                    "text string",
                    self.options.max_byte_length,
                )?;
                let bytes = self.read_bytes(length)?;
                match std::str::from_utf8(bytes) {
                    Ok(value) => Ok(CborValue::Text(value.to_owned())),
                    Err(_) => Err(CborError::cbor("CBOR text string contains invalid UTF-8")),
                }
            }
            4 => {
                let length = self.read_length(
                    additional_information,
                    "array",
                    self.options.max_container_length,
                )?;
                let mut result = Vec::new();
                for _ in 0..length {
                    result.push(self.read_item(depth + 1)?);
                }
                Ok(CborValue::Array(result))
            }
            5 => {
                let length = self.read_length(
                    additional_information,
                    "map",
                    self.options.max_container_length,
                )?;
                let mut result: Vec<(String, CborValue)> = Vec::new();
                let mut keys: HashSet<String> = HashSet::new();
                for _ in 0..length {
                    let key = match self.read_item(depth + 1)? {
                        CborValue::Text(key) => key,
                        _ => return Err(CborError::cbor("CBOR map keys must be strings")),
                    };
                    if !keys.insert(key.clone()) {
                        return Err(CborError::cbor("CBOR map contains a duplicate key"));
                    }
                    let value = self.read_item(depth + 1)?;
                    result.push((key, value));
                }
                Ok(CborValue::Map(result))
            }
            6 => Err(CborError::cbor("CBOR tags are not supported")),
            7 => self.read_simple(additional_information),
            _ => Err(CborError::cbor("Malformed CBOR major type")),
        }
    }

    fn read_simple(&mut self, additional_information: u8) -> Result<CborValue, CborError> {
        match additional_information {
            20 => Ok(CborValue::Bool(false)),
            21 => Ok(CborValue::Bool(true)),
            22 => Ok(CborValue::Null),
            27 => {
                let bytes = self.read_bytes(8)?;
                let value =
                    f64::from_be_bytes(bytes.try_into().expect("read_bytes returned eight bytes"));
                if !value.is_finite() {
                    return Err(CborError::cbor("Decoded CBOR number must be finite"));
                }
                if is_integer(value) && !is_safe_integer(value) {
                    return Err(CborError::cbor(
                        "Decoded CBOR integer is outside the safe range",
                    ));
                }
                Ok(CborValue::Number(value))
            }
            31 => Err(CborError::cbor("CBOR break marker is not supported")),
            _ => Err(CborError::cbor(
                "Unsupported CBOR simple value or floating-point width",
            )),
        }
    }

    fn read_length(
        &mut self,
        additional_information: u8,
        kind: &str,
        limit: u64,
    ) -> Result<u64, CborError> {
        if additional_information == 31 {
            return Err(CborError::cbor(format!(
                "Indefinite-length CBOR {kind}s are not supported"
            )));
        }
        let length = self.read_argument(additional_information)?;
        if length > limit {
            return Err(CborError::cbor(format!(
                "CBOR {kind} length exceeds configured limit of {limit}"
            )));
        }
        Ok(length)
    }

    fn read_argument(&mut self, additional_information: u8) -> Result<u64, CborError> {
        if additional_information < 24 {
            return Ok(u64::from(additional_information));
        }
        match additional_information {
            24 => Ok(u64::from(self.read_byte()?)),
            25 => {
                let bytes = self.read_bytes(2)?;
                Ok(u64::from(bytes[0]) * 0x100 + u64::from(bytes[1]))
            }
            26 => {
                let bytes = self.read_bytes(4)?;
                Ok(u64::from(bytes[0]) * 0x1_000_000
                    + u64::from(bytes[1]) * 0x1_0000
                    + u64::from(bytes[2]) * 0x100
                    + u64::from(bytes[3]))
            }
            27 => {
                let high = self.read_argument(26)?;
                let low = self.read_argument(26)?;
                if high > 0x1f_ffff {
                    return Err(CborError::cbor(
                        "Decoded CBOR integer or length is outside the safe range",
                    ));
                }
                Ok(high * UINT32_BASE + low)
            }
            31 => Err(CborError::cbor(
                "Indefinite-length CBOR items are not supported",
            )),
            _ => Err(CborError::cbor("Malformed CBOR additional information")),
        }
    }

    fn read_byte(&mut self) -> Result<u8, CborError> {
        if self.offset >= self.bytes.len() {
            return Err(CborError::cbor("Truncated CBOR payload"));
        }
        let value = self.bytes[self.offset];
        self.offset += 1;
        Ok(value)
    }

    fn read_bytes(&mut self, length: u64) -> Result<&'a [u8], CborError> {
        let remaining = (self.bytes.len() - self.offset) as u64;
        if length > remaining {
            return Err(CborError::cbor("Truncated CBOR payload"));
        }
        let length = length as usize;
        let value = &self.bytes[self.offset..self.offset + length];
        self.offset += length;
        Ok(value)
    }
}

/// Decodes exactly one item from the protocol's strict RFC 8949 subset.
pub fn decode_cbor(bytes: &[u8], options: Option<CborOptions>) -> Result<CborValue, CborError> {
    let resolved = resolve_options(options)?;
    if bytes.len() as u64 > resolved.max_byte_length {
        return Err(CborError::cbor(format!(
            "CBOR byte length exceeds configured limit of {}",
            resolved.max_byte_length
        )));
    }
    CborReader::new(bytes, resolved).decode()
}
