use crate::CryptoError;

/// Minimal RFC 8949 deterministic CBOR writer for TeleArk's explicit schema.
#[derive(Clone, Debug, Default)]
pub(crate) struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn map(&mut self, length: usize) -> Result<(), CryptoError> {
        self.head(5, usize_to_u64(length, "CBOR map length")?)
    }

    pub(crate) fn array(&mut self, length: usize) -> Result<(), CryptoError> {
        self.head(4, usize_to_u64(length, "CBOR array length")?)
    }

    pub(crate) fn unsigned(&mut self, value: u64) -> Result<(), CryptoError> {
        self.head(0, value)
    }

    pub(crate) fn signed(&mut self, value: i64) -> Result<(), CryptoError> {
        if value >= 0 {
            self.unsigned(value as u64)
        } else {
            self.head(1, (-1_i128 - i128::from(value)) as u64)
        }
    }

    pub(crate) fn byte_string(&mut self, value: &[u8]) -> Result<(), CryptoError> {
        self.head(2, usize_to_u64(value.len(), "CBOR byte string length")?)?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    pub(crate) fn text(&mut self, value: &str) -> Result<(), CryptoError> {
        self.head(3, usize_to_u64(value.len(), "CBOR text length")?)?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn head(&mut self, major: u8, value: u64) -> Result<(), CryptoError> {
        let prefix = major << 5;
        match value {
            0..=23 => self.bytes.push(prefix | value as u8),
            24..=0xff => {
                self.bytes.push(prefix | 24);
                self.bytes.push(value as u8);
            }
            0x100..=0xffff => {
                self.bytes.push(prefix | 25);
                self.bytes.extend_from_slice(&(value as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                self.bytes.push(prefix | 26);
                self.bytes.extend_from_slice(&(value as u32).to_be_bytes());
            }
            _ => {
                self.bytes.push(prefix | 27);
                self.bytes.extend_from_slice(&value.to_be_bytes());
            }
        }
        Ok(())
    }
}

fn usize_to_u64(value: usize, field: &'static str) -> Result<u64, CryptoError> {
    u64::try_from(value).map_err(|_| CryptoError::ArithmeticOverflow { field })
}

/// Strict deterministic-CBOR reader. It accepts only the data model used by
/// TeleArk and rejects indefinite lengths, non-shortest integers, tags, floats,
/// and simple values.
#[derive(Clone, Debug)]
pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    pub(crate) fn finish(self) -> Result<(), CryptoError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(CryptoError::TrailingData {
                context: "CBOR value",
            })
        }
    }

    pub(crate) fn map(&mut self, maximum: usize) -> Result<usize, CryptoError> {
        self.container_length(5, maximum, "CBOR map")
    }

    pub(crate) fn array(&mut self, maximum: usize) -> Result<usize, CryptoError> {
        self.container_length(4, maximum, "CBOR array")
    }

    pub(crate) fn map_key(&mut self, previous: &mut Option<u64>) -> Result<u64, CryptoError> {
        let key = self.unsigned()?;
        if previous.is_some_and(|old| key <= old) {
            return Err(CryptoError::DuplicateOrUnorderedMapKey);
        }
        *previous = Some(key);
        Ok(key)
    }

    pub(crate) fn unsigned(&mut self) -> Result<u64, CryptoError> {
        let (major, value) = self.head()?;
        if major != 0 {
            return Err(CryptoError::NonCanonicalCbor);
        }
        Ok(value)
    }

    pub(crate) fn u16(&mut self, field: &'static str) -> Result<u16, CryptoError> {
        u16::try_from(self.unsigned()?).map_err(|_| CryptoError::InvalidField { field })
    }

    pub(crate) fn u32(&mut self, field: &'static str) -> Result<u32, CryptoError> {
        u32::try_from(self.unsigned()?).map_err(|_| CryptoError::InvalidField { field })
    }

    pub(crate) fn signed(&mut self, field: &'static str) -> Result<i64, CryptoError> {
        let (major, value) = self.head()?;
        match major {
            0 => i64::try_from(value).map_err(|_| CryptoError::InvalidField { field }),
            1 => {
                if value > i64::MAX as u64 {
                    Err(CryptoError::InvalidField { field })
                } else {
                    Ok(-1 - value as i64)
                }
            }
            _ => Err(CryptoError::NonCanonicalCbor),
        }
    }

    pub(crate) fn byte_string(
        &mut self,
        maximum: usize,
        field: &'static str,
    ) -> Result<&'a [u8], CryptoError> {
        let (major, length) = self.head()?;
        if major != 2 {
            return Err(CryptoError::NonCanonicalCbor);
        }
        let length = bounded_length(length, maximum, field)?;
        self.take(length, field)
    }

    pub(crate) fn text(
        &mut self,
        maximum: usize,
        field: &'static str,
    ) -> Result<&'a str, CryptoError> {
        let (major, length) = self.head()?;
        if major != 3 {
            return Err(CryptoError::NonCanonicalCbor);
        }
        let length = bounded_length(length, maximum, field)?;
        let bytes = self.take(length, field)?;
        std::str::from_utf8(bytes).map_err(|_| CryptoError::InvalidUtf8 { field })
    }

    fn container_length(
        &mut self,
        expected_major: u8,
        maximum: usize,
        field: &'static str,
    ) -> Result<usize, CryptoError> {
        let (major, length) = self.head()?;
        if major != expected_major {
            return Err(CryptoError::NonCanonicalCbor);
        }
        bounded_length(length, maximum, field)
    }

    fn head(&mut self) -> Result<(u8, u64), CryptoError> {
        let first = *self
            .bytes
            .get(self.position)
            .ok_or(CryptoError::Truncated {
                context: "CBOR head",
            })?;
        self.position += 1;
        let major = first >> 5;
        let additional = first & 0x1f;
        let value = match additional {
            0..=23 => u64::from(additional),
            24 => {
                let value = u64::from(self.take(1, "CBOR u8")?[0]);
                if value < 24 {
                    return Err(CryptoError::NonCanonicalCbor);
                }
                value
            }
            25 => {
                let value = u64::from(u16::from_be_bytes(self.fixed_array("CBOR u16")?));
                if value <= 0xff {
                    return Err(CryptoError::NonCanonicalCbor);
                }
                value
            }
            26 => {
                let value = u64::from(u32::from_be_bytes(self.fixed_array("CBOR u32")?));
                if value <= 0xffff {
                    return Err(CryptoError::NonCanonicalCbor);
                }
                value
            }
            27 => {
                let value = u64::from_be_bytes(self.fixed_array("CBOR u64")?);
                if value <= 0xffff_ffff {
                    return Err(CryptoError::NonCanonicalCbor);
                }
                value
            }
            _ => return Err(CryptoError::NonCanonicalCbor),
        };
        Ok((major, value))
    }

    fn fixed_array<const N: usize>(
        &mut self,
        context: &'static str,
    ) -> Result<[u8; N], CryptoError> {
        self.take(N, context)?
            .try_into()
            .map_err(|_| CryptoError::Truncated { context })
    }

    fn take(&mut self, length: usize, context: &'static str) -> Result<&'a [u8], CryptoError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "CBOR cursor",
            })?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(CryptoError::Truncated { context })?;
        self.position = end;
        Ok(value)
    }
}

fn bounded_length(value: u64, maximum: usize, field: &'static str) -> Result<usize, CryptoError> {
    if value > maximum as u64 {
        return Err(CryptoError::LimitExceeded {
            field,
            limit: maximum as u64,
            actual: value,
        });
    }
    usize::try_from(value).map_err(|_| CryptoError::ArithmeticOverflow { field })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_integer_forms_roundtrip() {
        for value in [0, 23, 24, 255, 256, 65_535, 65_536, u32::MAX as u64 + 1] {
            let mut encoder = Encoder::default();
            assert!(encoder.unsigned(value).is_ok());
            let bytes = encoder.into_bytes();
            let mut decoder = Decoder::new(&bytes);
            assert_eq!(decoder.unsigned(), Ok(value));
            assert!(decoder.finish().is_ok());
        }
    }

    #[test]
    fn noncanonical_and_indefinite_encodings_are_rejected() {
        assert_eq!(
            Decoder::new(&[0x18, 0x17]).unsigned(),
            Err(CryptoError::NonCanonicalCbor)
        );
        assert_eq!(
            Decoder::new(&[0x9f]).array(10),
            Err(CryptoError::NonCanonicalCbor)
        );
        assert_eq!(
            Decoder::new(&[0xf9, 0, 0]).unsigned(),
            Err(CryptoError::NonCanonicalCbor)
        );
    }

    #[test]
    fn duplicate_and_reordered_keys_are_rejected() {
        let mut decoder = Decoder::new(&[1, 1]);
        let mut previous = None;
        assert_eq!(decoder.map_key(&mut previous), Ok(1));
        assert_eq!(
            decoder.map_key(&mut previous),
            Err(CryptoError::DuplicateOrUnorderedMapKey)
        );
    }
}
