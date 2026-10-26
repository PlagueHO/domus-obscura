pub(crate) fn is_canonical_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, value)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *value == b'-'
            } else {
                value.is_ascii_digit() || (b'a'..=b'f').contains(value)
            }
        })
}
