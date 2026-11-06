use symphonia::core::checksum::Crc16Ansi;
use symphonia::core::io::Monitor;

pub(super) fn frames(
    bitrates: &[u32],
    protected: bool,
    padded: bool,
    rate_index: u8,
    mono: bool,
) -> Vec<u8> {
    let rates = [44_100, 48_000, 32_000];
    let mut bytes = Vec::new();
    for bitrate in bitrates {
        let index = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
        .iter()
        .position(|value| value == bitrate)
        .unwrap_or_else(|| panic!("unsupported generated fixture bitrate"));
        let length = 144_000 * bitrate / rates[usize::from(rate_index)] + u32::from(padded);
        let mut frame = vec![0; length as usize];
        frame[..4].copy_from_slice(&[
            0xff,
            if protected { 0xfa } else { 0xfb },
            ((index as u8) << 4) | (rate_index << 2) | (u8::from(padded) << 1),
            if mono { 0xc0 } else { 0 },
        ]);
        if protected {
            let mut crc = Crc16Ansi::new(0xffff);
            crc.process_buf_bytes(&frame[2..4]);
            crc.process_buf_bytes(&frame[6..if mono { 23 } else { 38 }]);
            frame[4..6].copy_from_slice(&crc.crc().to_be_bytes());
        }
        bytes.extend(frame);
    }
    bytes
}
