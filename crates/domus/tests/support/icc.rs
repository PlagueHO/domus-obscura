/// Generate an ICC fixture with four-byte-aligned tag data. The moxcms writer
/// emits tag payloads without inter-tag padding, so frame the table explicitly.
pub fn profile(profile: moxcms::ColorProfile) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let encoded = profile.encode()?;
    let word = |offset: usize| -> Result<usize, Box<dyn std::error::Error>> {
        let bytes: [u8; 4] = encoded
            .get(offset..offset + 4)
            .ok_or("ICC word")?
            .try_into()?;
        Ok(usize::try_from(u32::from_be_bytes(bytes))?)
    };
    let count = word(128)?;
    let mut output = encoded[..132 + count * 12].to_vec();
    for index in 0..count {
        let tag = 132 + index * 12;
        let source = word(tag + 4)?;
        let length = word(tag + 8)?;
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
        let offset = u32::try_from(output.len())?;
        output[tag + 4..tag + 8].copy_from_slice(&offset.to_be_bytes());
        output.extend_from_slice(encoded.get(source..source + length).ok_or("ICC tag")?);
    }
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
    let size = u32::try_from(output.len())?;
    output[..4].copy_from_slice(&size.to_be_bytes());
    Ok(output)
}
