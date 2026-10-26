---
applyTo: "crates/domus/**/*.rs"
---

# Media and provider invariants

## Provider requests

- Require the durable single-use CatalogPermit for every MusicBrainz HTTP request.
- Use that same catalog budget for each CAA redirect hop. Keep exact listing membership, release, archive item and filename bindings. Never follow arbitrary artwork URLs or HTTP downgrades.
- Keep provider edit approval separate from user approval and rights decisions. Transport hashes do not prove valid images; retain strict normalization.
- Keep its OS lock through response persistence. Use saved intervals and monotonic clocks for provider backoff, not wall-clock deadlines.
- Preserve longer provider backoff. Never automatically clear an unsupported retry pause.
- Route image retrieval through ImageService and bounded ImageProcessor work. Recheck fresh exact-release evidence and frozen processing options before persistence.

## Audio and image output

- Check every MP3 audio frame for actual 320 kbps and complete decoding. Keep the optional first metadata frame separate; never substitute average or advertised bitrate.
- Independently check FLAC coding boundaries, reconstructed sample ranges and frame CRCs before decoding. Require a supplied MD5 to match; keep an absent MD5 explicitly unverified.
- Write fresh metadata only to empty private staging. Derive tags from the frozen exact release, preserve encoded audio, bind the full input hash and verify the complete written output before saving a receipt.
- Keep JPEG container checks and warning-free native decoding together. Never accept a repaired image or remove the scan, coding, color and allocation limits.
