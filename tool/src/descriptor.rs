//! The bug, as seen from the host: in the UAC2 configuration descriptor, Output Terminal 22
//! (USB streaming, i.e. the capture stream) has bSourceID 2 (the USB playback input terminal)
//! instead of 1 (the Microphone input terminal). Windows' usbaudio2 then finds no physical
//! input behind the capture pin and creates no Recording endpoint.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorState {
    /// OT22.bSourceID == 2: the factory firmware bug is present.
    Buggy,
    /// OT22.bSourceID == 1: fixed firmware is running.
    Fixed,
    /// OT22.bSourceID is something else entirely.
    Other(u8),
    /// No UAC2 Output Terminal 22 was found in the descriptor.
    NotFound,
}

impl DescriptorState {
    pub fn describe(&self) -> &'static str {
        match self {
            DescriptorState::Buggy => "BUG PRESENT: capture terminal is wired to USB playback (bSourceID 2)",
            DescriptorState::Fixed => "FIXED: capture terminal is wired to the Microphone (bSourceID 1)",
            DescriptorState::Other(_) => "unexpected capture terminal wiring",
            DescriptorState::NotFound => "no UAC2 output terminal 22 found (not an M-Track 2X2 audio configuration?)",
        }
    }
}

/// Walk a configuration descriptor and inspect Output Terminal 22.
pub fn capture_terminal_state(config: &[u8]) -> DescriptorState {
    let mut i = 0;
    while i + 2 <= config.len() {
        let len = config[i] as usize;
        if len < 2 || i + len > config.len() {
            break;
        }
        let d = &config[i..i + len];
        // CS_INTERFACE (0x24), OUTPUT_TERMINAL (0x03), bTerminalID 22, wTerminalType 0x0101 (USB streaming)
        if d[1] == 0x24 && len >= 12 && d[2] == 0x03 && d[3] == 22 && d[4] == 0x01 && d[5] == 0x01 {
            return match d[7] {
                1 => DescriptorState::Fixed,
                2 => DescriptorState::Buggy,
                x => DescriptorState::Other(x),
            };
        }
        i += len;
    }
    DescriptorState::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_terminal() {
        let mut cfg = vec![9, 2, 0, 0, 1, 1, 0, 0x80, 0x32];
        cfg.extend_from_slice(&[0x0c, 0x24, 0x03, 0x16, 0x01, 0x01, 0x00, 0x02, 0x28, 0x00, 0x00, 0x07]);
        assert_eq!(capture_terminal_state(&cfg), DescriptorState::Buggy);
        cfg[9 + 7] = 1;
        assert_eq!(capture_terminal_state(&cfg), DescriptorState::Fixed);
        assert_eq!(capture_terminal_state(&cfg[..9]), DescriptorState::NotFound);
    }
}
