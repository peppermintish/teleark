//! User-selected transport limits. Protocol/server deadlines are never overridden.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferTuning {
    pub upload_tasks: u16,
    pub upload_parts: u16,
    pub upload_connections: u16,
    pub upload_queue: u16,
    pub upload_attempts: u16,
    pub download_tasks: u16,
    pub download_parts: u16,
    pub download_connections: u16,
    pub download_attempts: u16,
}
impl Default for TransferTuning {
    fn default() -> Self {
        Self {
            upload_tasks: 2,
            upload_parts: 16,
            upload_connections: 8,
            upload_queue: 2,
            upload_attempts: 4,
            download_tasks: 2,
            download_parts: 16,
            download_connections: 8,
            download_attempts: 4,
        }
    }
}
impl TransferTuning {
    pub fn values(self) -> [u16; 9] {
        [
            self.upload_tasks,
            self.upload_parts,
            self.upload_connections,
            self.upload_queue,
            self.upload_attempts,
            self.download_tasks,
            self.download_parts,
            self.download_connections,
            self.download_attempts,
        ]
    }
    pub const BOUNDS: [(u16, u16); 9] = [
        (1, 8),
        (2, 64),
        (1, 8),
        (1, 16),
        (1, 10),
        (1, 8),
        (1, 64),
        (1, 8),
        (1, 10),
    ];
    pub fn from_values(v: [u16; 9]) -> Option<Self> {
        if !v
            .iter()
            .zip(Self::BOUNDS)
            .all(|(&n, (min, max))| (min..=max).contains(&n))
        {
            return None;
        }
        Some(Self {
            upload_tasks: v[0],
            upload_parts: v[1],
            upload_connections: v[2],
            upload_queue: v[3],
            upload_attempts: v[4],
            download_tasks: v[5],
            download_parts: v[6],
            download_connections: v[7],
            download_attempts: v[8],
        })
    }
    /// Explicit v1 codec. Missing settings migrate to defaults; malformed/newer values fail closed.
    pub fn encode(self) -> String {
        self.values().map(|v| v.to_string()).join(",")
    }
    pub fn decode(value: &str) -> Option<Self> {
        let values = value
            .split(',')
            .map(str::parse::<u16>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Self::from_values(values.try_into().ok()?)
    }
    pub fn validate(self) -> bool {
        Self::from_values(self.values()).is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_codec_and_bounds() {
        let value = TransferTuning::default();
        assert_eq!(
            (
                value.upload_tasks,
                value.upload_parts,
                value.upload_connections
            ),
            (2, 16, 8)
        );
        assert_eq!(
            (
                value.download_tasks,
                value.download_parts,
                value.download_connections
            ),
            (2, 16, 8)
        );
        assert_eq!(TransferTuning::decode(&value.encode()), Some(value));
        for input in [
            "",
            "3,10",
            "3,1,2,2,4,3,8,2,4",
            "3,10,2,999,4,3,8,2,4",
            "3,10,2,2,4,3,8,2,4,1",
        ] {
            assert_eq!(TransferTuning::decode(input), None);
        }
    }
}
