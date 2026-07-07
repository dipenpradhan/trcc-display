//! Wire protocol for Thermalright "Digital" LED/segment coolers (USB `0416:8001`).
//!
//! This is a clean-room reimplementation of the LED HID protocol (the same one
//! [`trrc-linux`](https://github.com/Lexonight1/thermalright-trcc-linux) speaks).
//! Everything here is pure — no I/O — so it is trivially testable and shared
//! between the USB layer and the renderer.
//!
//! # Packets
//!
//! HID, 64-byte reports. Two commands share the `DA DB DC DD` magic:
//!
//! * **init** (`cmd = 1`): a single 64-byte report, magic + `cmd` at byte 12.
//!   The device answers once per power cycle with its identity (`PM`/`SUB`).
//! * **data** (`cmd = 2`): a 20-byte header (magic, `cmd`, `u16` payload length
//!   at offset 16) followed by `n*3` RGB bytes, then chunked into 64-byte writes.
//!
//! Each color channel is scaled by `0.4` on the wire (the cooler's hardware
//! perceptual scale); off LEDs are sent as `(0, 0, 0)`.
//!
//! # 7-segment font
//!
//! Segments are labelled `a..g` in the standard layout. [`WIRE_ORDER`] is the
//! order the per-digit LED index arrays use, so segment `a` = `digit_leds[0]`,
//! etc.
//!
//! ```text
//!       aaa
//!      f   b
//!      f   b
//!       ggg
//!      e   c
//!      e   c
//!       ddd
//! ```

/// USB endpoints (HID interrupt).
pub const EP_WRITE: u8 = 0x02;
/// USB IN endpoint for handshake responses.
pub const EP_READ: u8 = 0x81;

/// One HID report is 64 bytes; packets are chunked to this size on the wire.
pub const HID_REPORT_SIZE: usize = 64;
const HEADER_SIZE: usize = 20;

const MAGIC: [u8; 4] = [0xDA, 0xDB, 0xDC, 0xDD];
const CMD_INIT: u8 = 1;
const CMD_DATA: u8 = 2;

/// Hardware perceptual scale applied to every channel on the wire.
const COLOR_SCALE: f32 = 0.4;

/// An RGB triple in logical (0-255, unscaled) space.
///
/// This is the color space used throughout the render pipeline. On the wire,
/// each channel is scaled by [`COLOR_SCALE`] (0.4) before transmission.
///
/// # Conversions
///
/// ```
/// use trcc_display::protocol::Rgb;
///
/// let rgb = Rgb(255, 128, 0);
/// let arr: [u8; 3] = rgb.into();
/// assert_eq!(arr, [255, 128, 0]);
///
/// let back: Rgb = [255, 128, 0].into();
/// assert_eq!(back, rgb);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<[u8; 3]> for Rgb {
    fn from([r, g, b]: [u8; 3]) -> Self {
        Self(r, g, b)
    }
}

impl From<Rgb> for [u8; 3] {
    fn from(rgb: Rgb) -> Self {
        [rgb.0, rgb.1, rgb.2]
    }
}

/// Build the 64-byte init/handshake report.
///
/// This packet triggers the device to respond with its identity bytes.
/// The device only answers once per power cycle; subsequent init packets
/// return garbage, so use [`crate::usb::UsbConfig::cache_path`] to cache
/// the result.
pub fn init_packet() -> [u8; HID_REPORT_SIZE] {
    let mut buf = [0u8; HID_REPORT_SIZE];
    buf[0..4].copy_from_slice(&MAGIC);
    buf[12] = CMD_INIT;
    buf
}

/// Extract `(pm, sub)` from a handshake response, validating length + magic.
///
/// `PM` (byte 5) selects the device *style* (which profile to use); `SUB`
/// (byte 4) is a wire-remap sub-variant. Returns `None` if the response is
/// too short to contain them.
///
/// # Arguments
///
/// * `resp` — raw bytes from the HID interrupt read (must be ≥ 7 bytes).
///
/// # Returns
///
/// `Some((pm, sub))` on success, `None` if the buffer is too short.
pub fn parse_handshake(resp: &[u8]) -> Option<(u8, u8)> {
    if resp.len() < 7 {
        return None;
    }
    if resp[0..4] != MAGIC {
        tracing::warn!(magic = %hex4(resp), "handshake: unexpected magic (accepting anyway)");
    }
    Some((resp[5], resp[4]))
}

fn hex4(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for x in b.iter().take(4) {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// Build a full data packet from `colors` (already in physical/wire order).
///
/// `colors.len()` must equal the profile's LED count. Channels are scaled by
/// [`COLOR_SCALE`] (0.4); the returned bytes are header + body, *not yet*
/// chunked (see [`chunks`]).
///
/// # Returns
///
/// A `Vec<u8>` of length `HEADER_SIZE + colors.len() * 3`.
///
/// # Panics
///
/// Does not panic. Empty color arrays produce a 20-byte header with zero payload.
pub fn data_packet(colors: &[Rgb]) -> Vec<u8> {
    let payload_len = colors.len() * 3;
    let mut buf = Vec::with_capacity(HEADER_SIZE + payload_len);
    buf.extend_from_slice(&MAGIC);
    buf.resize(HEADER_SIZE, 0);
    buf[12] = CMD_DATA;
    let plen = payload_len as u16;
    buf[16..18].copy_from_slice(&plen.to_le_bytes());
    for color in colors {
        buf.push(scale(color.0));
        buf.push(scale(color.1));
        buf.push(scale(color.2));
    }
    buf
}

fn scale(channel: u8) -> u8 {
    (channel as f32 * COLOR_SCALE) as u8
}

/// Split a packet into zero-padded 64-byte HID reports for writing.
///
/// Each report is exactly [`HID_REPORT_SIZE`] bytes. The last report may be
/// zero-padded if the packet size is not a multiple of 64.
///
/// # Arguments
///
/// * `packet` — a full data packet from [`data_packet`].
///
/// # Returns
///
/// A vector of 64-byte arrays ready for `write_interrupt` calls.
pub fn chunks(packet: &[u8]) -> Vec<[u8; HID_REPORT_SIZE]> {
    packet
        .chunks(HID_REPORT_SIZE)
        .map(|c| {
            let mut report = [0u8; HID_REPORT_SIZE];
            report[..c.len()].copy_from_slice(c);
            report
        })
        .collect()
}

// ── 7-segment font ─────────────────────────────────────────────────────────
//
// Segments are labelled a..g in the standard layout; `WIRE_ORDER` is the order
// the per-digit LED index arrays use, so segment `a` = `digit_leds[0]`, etc.
//
//      aaa
//     f   b
//     f   b
//      ggg
//     e   c
//     e   c
//      ddd

/// Segment bitmask order for a digit's 7 LED indices.
///
/// Index 0 = segment `a`, index 1 = segment `b`, …, index 6 = segment `g`.
pub const WIRE_ORDER: [char; 7] = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];

/// Return, for `ch`, which of the 7 wire positions are lit (`true` = on).
///
/// Unknown characters render blank. Supports digits plus a few glyphs the
/// coolers use (`C`, `F`, `H`, `G`, space).
///
/// # Arguments
///
/// * `ch` — the character to render. Digits `0-9` and the special characters
///   `C`, `F`, `H`, `G`, and space are supported.
///
/// # Returns
///
/// A `[bool; 7]` where index `i` corresponds to segment
/// [`WIRE_ORDER`][`i`].
pub fn seven_seg(ch: char) -> [bool; 7] {
    // Bitfield: bit i (from 'a') set = segment lit.
    let segs: &str = match ch {
        '0' => "abcdef",
        '1' => "bc",
        '2' => "abdeg",
        '3' => "abcdg",
        '4' => "bcfg",
        '5' => "acdfg",
        '6' => "acdefg",
        '7' => "abc",
        '8' => "abcdefg",
        '9' | 'G' => "abcdfg",
        'C' => "adef",
        'F' => "aefg",
        'H' => "bcefg",
        _ => "", // space / unknown → blank
    };
    let mut out = [false; 7];
    for (i, c) in WIRE_ORDER.iter().enumerate() {
        out[i] = segs.contains(*c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── init_packet ────────────────────────────────────────────────────────

    #[test]
    fn init_packet_shape() {
        let p = init_packet();
        assert_eq!(p.len(), HID_REPORT_SIZE);
        assert_eq!(&p[0..4], &MAGIC);
        assert_eq!(p[12], CMD_INIT);
        assert!(p[13..].iter().all(|&b| b == 0));
    }

    #[test]
    fn init_packet_is_zero_elsewhere() {
        let p = init_packet();
        for (i, &b) in p.iter().enumerate() {
            if i == 0 || i == 1 || i == 2 || i == 3 || i == 12 {
                continue; // magic + cmd
            }
            assert_eq!(b, 0, "byte {i} should be zero");
        }
    }

    // ── parse_handshake ────────────────────────────────────────────────────

    #[test]
    fn handshake_extracts_pm_sub() {
        let mut resp = [0u8; 64];
        resp[0..4].copy_from_slice(&MAGIC);
        resp[4] = 9; // sub
        resp[5] = 3; // pm
        assert_eq!(parse_handshake(&resp), Some((3, 9)));
    }

    #[test]
    fn handshake_too_short_is_none() {
        assert_eq!(parse_handshake(&[0xDA, 0xDB]), None);
    }

    #[test]
    fn handshake_exactly_7_bytes() {
        let mut resp = [0u8; 7];
        resp[0..4].copy_from_slice(&MAGIC);
        resp[4] = 1;
        resp[5] = 2;
        assert_eq!(parse_handshake(&resp), Some((2, 1)));
    }

    #[test]
    fn handshake_accepts_bad_magic_with_warning() {
        let resp = [0xFF, 0xFF, 0xFF, 0xFF, 10, 20, 30];
        // Should still extract PM=20, SUB=10 even with bad magic.
        assert_eq!(parse_handshake(&resp), Some((20, 10)));
    }

    // ── Rgb ────────────────────────────────────────────────────────────────

    #[test]
    fn rgb_from_array() {
        let rgb: Rgb = [255, 128, 64].into();
        assert_eq!(rgb.0, 255);
        assert_eq!(rgb.1, 128);
        assert_eq!(rgb.2, 64);
    }

    #[test]
    fn rgb_to_array() {
        let rgb = Rgb(10, 20, 30);
        let arr: [u8; 3] = rgb.into();
        assert_eq!(arr, [10, 20, 30]);
    }

    #[test]
    fn rgb_roundtrip() {
        let original = Rgb(0, 255, 128);
        let arr: [u8; 3] = original.into();
        let back: Rgb = arr.into();
        assert_eq!(back, original);
    }

    #[test]
    fn rgb_default_is_black() {
        let rgb = Rgb::default();
        assert_eq!(rgb.0, 0);
        assert_eq!(rgb.1, 0);
        assert_eq!(rgb.2, 0);
    }

    #[test]
    fn rgb_equality() {
        assert_eq!(Rgb(0, 0, 0), Rgb(0, 0, 0));
        assert_ne!(Rgb(0, 0, 0), Rgb(0, 0, 1));
    }

    #[test]
    fn rgb_copy_works() {
        let a = Rgb(1, 2, 3);
        let b = a;
        assert_eq!(a, b);
    }

    // ── data_packet ────────────────────────────────────────────────────────

    #[test]
    fn data_packet_header_and_scale() {
        let colors = [Rgb(255, 0, 0), Rgb(0, 255, 0)];
        let pkt = data_packet(&colors);
        assert_eq!(&pkt[0..4], &MAGIC);
        assert_eq!(pkt[12], CMD_DATA);
        // payload length = 2 colors * 3 = 6, little-endian at offset 16.
        assert_eq!(&pkt[16..18], &6u16.to_le_bytes());
        assert_eq!(pkt.len(), HEADER_SIZE + 6);
        // 255 * 0.4 = 102 on the wire; 0 stays 0.
        assert_eq!(pkt[HEADER_SIZE], 102);
        assert_eq!(pkt[HEADER_SIZE + 1], 0);
        assert_eq!(pkt[HEADER_SIZE + 3 + 1], 102);
    }

    #[test]
    fn data_packet_empty() {
        let pkt = data_packet(&[]);
        assert_eq!(&pkt[0..4], &MAGIC);
        assert_eq!(pkt[12], CMD_DATA);
        assert_eq!(&pkt[16..18], &0u16.to_le_bytes());
        assert_eq!(pkt.len(), HEADER_SIZE);
    }

    #[test]
    fn data_packet_single_led() {
        let colors = [Rgb(128, 64, 32)];
        let pkt = data_packet(&colors);
        assert_eq!(pkt.len(), HEADER_SIZE + 3);
        // 128 * 0.4 = 51, 64 * 0.4 = 25, 32 * 0.4 = 12
        assert_eq!(pkt[HEADER_SIZE], 51);
        assert_eq!(pkt[HEADER_SIZE + 1], 25);
        assert_eq!(pkt[HEADER_SIZE + 2], 12);
    }

    #[test]
    fn data_packet_max_value_scaled() {
        let colors = [Rgb(255, 255, 255)];
        let pkt = data_packet(&colors);
        // 255 * 0.4 = 102
        assert_eq!(pkt[HEADER_SIZE], 102);
        assert_eq!(pkt[HEADER_SIZE + 1], 102);
        assert_eq!(pkt[HEADER_SIZE + 2], 102);
    }

    #[test]
    fn data_packet_zero_stays_zero() {
        let colors = [Rgb(0, 0, 0)];
        let pkt = data_packet(&colors);
        assert_eq!(pkt[HEADER_SIZE], 0);
        assert_eq!(pkt[HEADER_SIZE + 1], 0);
        assert_eq!(pkt[HEADER_SIZE + 2], 0);
    }

    // ── chunks ─────────────────────────────────────────────────────────────

    #[test]
    fn chunks_pad_to_report_size() {
        let packet = vec![0xAAu8; 272]; // 84 LEDs * 3 + 20 header
        let reports = chunks(&packet);
        assert_eq!(reports.len(), 5); // ceil(272 / 64)
        assert!(reports.iter().all(|r| r.len() == HID_REPORT_SIZE));
        // last report: 272 % 64 = 16 real bytes, rest zero-padded.
        assert_eq!(reports[4][15], 0xAA);
        assert_eq!(reports[4][16], 0x00);
    }

    #[test]
    fn chunks_exact_multiple() {
        let packet = vec![0x55u8; 128]; // exactly 2 * 64
        let reports = chunks(&packet);
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|r| r.iter().all(|&b| b == 0x55)));
    }

    #[test]
    fn chunks_empty_packet() {
        let reports = chunks(&[]);
        assert!(reports.is_empty());
    }

    #[test]
    fn chunks_single_byte() {
        let reports = chunks(&[0x42]);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0][0], 0x42);
        assert_eq!(reports[0][1], 0x00);
    }

    #[test]
    fn chunks_preserves_content() {
        let mut packet = vec![0u8; 200];
        for (i, b) in packet.iter_mut().enumerate() {
            *b = i as u8;
        }
        let reports = chunks(&packet);
        // Reassemble and compare.
        let mut reassembled = Vec::new();
        for r in &reports {
            reassembled.extend_from_slice(r);
        }
        // Trim trailing zeros (padding).
        reassembled.truncate(packet.len());
        assert_eq!(reassembled, packet);
    }

    // ── seven_seg ──────────────────────────────────────────────────────────

    #[test]
    fn seven_seg_digits() {
        assert_eq!(seven_seg('8'), [true; 7]); // all segments
        assert_eq!(seven_seg(' '), [false; 7]); // blank
        // '1' lights only b, c (wire positions 1 and 2).
        assert_eq!(
            seven_seg('1'),
            [false, true, true, false, false, false, false]
        );
    }

    #[test]
    fn seven_seg_all_digits() {
        // Verify each digit 0-9 has the expected segment pattern.
        let patterns: [(&str, [bool; 7]); 10] = [
            ("0", [true, true, true, true, true, true, false]),
            ("1", [false, true, true, false, false, false, false]),
            ("2", [true, true, false, true, true, false, true]),
            ("3", [true, true, true, true, false, false, true]),
            ("4", [false, true, true, false, false, true, true]),
            ("5", [true, false, true, true, false, true, true]),
            ("6", [true, false, true, true, true, true, true]),
            ("7", [true, true, true, false, false, false, false]),
            ("8", [true; 7]),
            ("9", [true, true, true, true, false, true, true]),
        ];
        for (digit, expected) in &patterns {
            let ch = digit.chars().next().unwrap();
            let actual = seven_seg(ch);
            assert_eq!(
                actual, *expected,
                "digit '{digit}' should be {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn seven_seg_special_chars() {
        // C = "adef" (wire 0,3,4,5)
        assert_eq!(
            seven_seg('C'),
            [true, false, false, true, true, true, false]
        );
        // F = "aefg" (wire 0,4,5,6)
        assert_eq!(
            seven_seg('F'),
            [true, false, false, false, true, true, true]
        );
        // H = "bcefg" (wire 1,2,4,5,6)
        assert_eq!(seven_seg('H'), [false, true, true, false, true, true, true]);
        // G = same as '9' = "abcdfg"
        assert_eq!(seven_seg('G'), seven_seg('9'));
    }

    #[test]
    fn seven_seg_unknown_is_blank() {
        assert_eq!(seven_seg('X'), [false; 7]);
        assert_eq!(seven_seg('z'), [false; 7]);
        assert_eq!(seven_seg(':'), [false; 7]);
    }

    // ── WIRE_ORDER ────────────────────────────────────────────────────────

    #[test]
    fn wire_order_is_seven_chars() {
        assert_eq!(WIRE_ORDER.len(), 7);
        assert_eq!(WIRE_ORDER, ['a', 'b', 'c', 'd', 'e', 'f', 'g']);
    }
}
