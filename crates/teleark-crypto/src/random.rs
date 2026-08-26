use crate::CryptoError;

/// Injectable random byte source. Production uses [`OsRandom`].
pub trait RandomSource {
    fn fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), CryptoError>;
}

/// Operating-system cryptographically secure random source.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), CryptoError> {
        getrandom::fill(destination).map_err(|_| CryptoError::RandomSourceFailed)
    }
}

/// Deterministic test-only byte source. This is deliberately unavailable from
/// normal production builds and is not cryptographically secure.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug)]
pub struct DeterministicRandom {
    state: u64,
}

#[cfg(any(test, feature = "test-support"))]
impl DeterministicRandom {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }
}

#[cfg(any(test, feature = "test-support"))]
impl RandomSource for DeterministicRandom {
    fn fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), CryptoError> {
        for chunk in destination.chunks_mut(8) {
            let bytes = self.next_u64().to_be_bytes();
            let length = chunk.len();
            chunk.copy_from_slice(&bytes[..length]);
        }
        Ok(())
    }
}
