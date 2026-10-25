use super::*;
use std::io::{Cursor, Read};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn decode(reader: &mut impl Read) -> Result<(u32, Vec<u8>)> {
    read_frame(|buffer| reader.read_exact(buffer).map_err(Error::from))
}

#[test]
fn normal_frames_preserve_codes_and_boundaries_without_a_socket() -> TestResult {
    let mut bytes = Vec::new();
    send(&mut bytes, 26, b"search")?;
    send(&mut bytes, u32::MAX, b"future-message")?;
    send(&mut bytes, 32, b"")?;
    assert_eq!(&bytes[..8], &[10, 0, 0, 0, 26, 0, 0, 0]);
    let mut reader = Cursor::new(bytes);
    assert_eq!(decode(&mut reader)?, (26, b"search".to_vec()));
    assert_eq!(decode(&mut reader)?, (u32::MAX, b"future-message".to_vec()));
    assert_eq!(decode(&mut reader)?, (32, Vec::new()));
    assert!(decode(&mut reader).is_err());
    Ok(())
}

#[test]
fn normal_frame_limits_reject_before_body_read_or_write() -> TestResult {
    for size in [0u32, 1, 3, MAX_FRAME as u32 + 1, u32::MAX] {
        let mut reader = Cursor::new(size.to_le_bytes());
        assert!(decode(&mut reader).is_err());
        assert_eq!(reader.position(), 4);
    }
    let mut bytes = Vec::new();
    assert!(send(&mut bytes, 26, &vec![0; MAX_FRAME - 3]).is_err());
    assert!(bytes.is_empty());
    send(&mut bytes, 26, &vec![7; MAX_FRAME - 4])?;
    let (code, body) = decode(&mut Cursor::new(bytes))?;
    assert_eq!(code, 26);
    assert_eq!(body.len(), MAX_FRAME - 4);
    assert!(body.iter().all(|byte| *byte == 7));
    Ok(())
}

#[test]
fn truncated_frames_and_transport_check_failures_are_not_hidden() -> TestResult {
    let mut bytes = Vec::new();
    send(&mut bytes, 9, b"bounded payload")?;
    for count in 0..bytes.len() {
        assert!(decode(&mut Cursor::new(&bytes[..count])).is_err());
    }
    let mut reader = Cursor::new(&bytes);
    let mut reads = 0;
    let result = read_frame(|buffer| {
        reads += 1;
        if reads == 3 {
            return Err(Error::Conflict("ownership check failed".into()));
        }
        reader.read_exact(buffer)?;
        Ok(())
    });
    assert!(matches!(result, Err(Error::Conflict(message)) if message == "ownership check failed"));
    assert_eq!(reader.position(), 8);
    Ok(())
}

#[test]
fn peer_init_uses_one_byte_codes_and_checks_length_before_writing() -> TestResult {
    let mut bytes = Vec::new();
    send_init(&mut bytes, 0, &123u32.to_le_bytes())?;
    assert_eq!(bytes, [5, 0, 0, 0, 0, 123, 0, 0, 0]);
    bytes.clear();
    let mut body = Vec::new();
    string(&mut body, "fixture-user");
    string(&mut body, "P");
    body.extend_from_slice(&0u32.to_le_bytes());
    send_init(&mut bytes, 1, &body)?;
    assert_eq!(
        u32::from_le_bytes(bytes[..4].try_into()?) as usize,
        body.len() + 1
    );
    assert_eq!(bytes[4], 1);
    assert_eq!(&bytes[5..], &body);
    bytes.clear();
    assert!(send_init(&mut bytes, 1, &vec![0; MAX_FRAME]).is_err());
    assert!(bytes.is_empty());
    Ok(())
}

#[test]
fn field_decoder_preserves_values_and_rejects_invalid_or_unconsumed_input() -> TestResult {
    let mut bytes = vec![1];
    bytes.extend_from_slice(&17u32.to_le_bytes());
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    string(&mut bytes, "fixture");
    let mut fields = Fields(&bytes);
    assert!(fields.boolean()?);
    assert_eq!(fields.u32()?, 17);
    assert_eq!(fields.u64()?, u64::MAX);
    assert_eq!(fields.text(7)?, "fixture");
    fields.end()?;
    assert!(!Fields(&[0]).boolean()?);
    assert!(Fields(&[2]).boolean().is_err());
    assert!(Fields(&[0; 3]).u32().is_err());
    assert!(Fields(&[0; 7]).u64().is_err());
    assert!(Fields(&[0]).end().is_err());
    for bytes in [
        vec![8, 0, 0, 0, b'a'],
        vec![1, 0, 0, 0, 0xff],
        vec![u8::MAX; 4],
    ] {
        assert!(Fields(&bytes).text(7).is_err());
    }
    Ok(())
}
