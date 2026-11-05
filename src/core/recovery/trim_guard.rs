#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrimState {
    ActiveData,
    ZeroFilled,
    PatternFilled(u8),
}

/// Evaluates if a buffer is discarded by SSD TRIM / Deallocate
pub fn evaluate_trim_state(buffer: &[u8]) -> TrimState {
    if buffer.is_empty() {
        return TrimState::ActiveData;
    }

    let first = buffer[0];
    if buffer.iter().all(|&b| b == first) {
        if first == 0x00 {
            TrimState::ZeroFilled
        } else {
            TrimState::PatternFilled(first)
        }
    } else {
        TrimState::ActiveData
    }
}

/// Fast entropy check to abort redundant carving passes on synthetic TRIM zeroes
pub struct TrimGuard {
    consecutive_zero_blocks: usize,
    threshold_blocks: usize,
    pub is_ssd: bool,
}

#[allow(dead_code)]
impl TrimGuard {
    pub fn new(is_ssd: bool, threshold_blocks: usize) -> Self {
        Self {
            consecutive_zero_blocks: 0,
            threshold_blocks,
            is_ssd,
        }
    }

    /// Returns true if TRIM discard bail condition is triggered
    pub fn inspect_block(&mut self, buffer: &[u8]) -> bool {
        if !self.is_ssd {
            return false;
        }

        match evaluate_trim_state(buffer) {
            TrimState::ZeroFilled | TrimState::PatternFilled(_) => {
                self.consecutive_zero_blocks += 1;
                self.consecutive_zero_blocks >= self.threshold_blocks
            }
            TrimState::ActiveData => {
                self.consecutive_zero_blocks = 0;
                false
            }
        }
    }

    pub fn reset(&mut self) {
        self.consecutive_zero_blocks = 0;
    }
}
