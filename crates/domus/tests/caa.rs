use domus::infrastructure::caa::{CaaListing, CaaRequest, MAX_CAA_IMAGES, MAX_CAA_LISTING};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const OTHER: &str = "20000000-0000-4000-8000-000000000002";

fn image(id: &str) -> Value {
    json!({
        "id": id,
        "edit": 9007199254740993u64,
        "image": format!("http://coverartarchive.org/release/{RELEASE}/{id}.png"),
        "types": ["Front"],
        "front": true,
        "back": false,
        "approved": false,
        "comment": "Original <&> artwork\nSecond line",
        "thumbnails": {"1200": "https://untrusted.example/never-request-this"}
    })
}

fn listing() -> Value {
    json!({
        "release": format!("http://musicbrainz.org/release/{RELEASE}"),
        "images": [image("9007199254740995")]
    })
}

fn parse(raw: &Value) -> Result<CaaListing, Box<dyn std::error::Error>> {
    Ok(CaaListing::parse(RELEASE, &serde_json::to_vec(raw)?)?)
}

#[test]
fn caa_listing_keeps_exact_release_ids_order_labels_and_separate_provider_approval() -> TestResult {
    let mut raw = listing();
    raw["images"].as_array_mut().ok_or("missing images")?.push({
        let mut image = image("9007199254740997");
        image["types"] = json!(["Front", "Booklet", "A future type"]);
        image["front"] = json!(false);
        image["approved"] = json!(true);
        image
    });
    let body = serde_json::to_vec(&raw)?;
    let parsed = CaaListing::parse(RELEASE, &body)?;
    assert_eq!(parsed.release_id(), RELEASE);
    assert_eq!(
        parsed.source(),
        format!("https://coverartarchive.org/release/{RELEASE}/")
    );
    assert_eq!(
        parsed.payload_sha256(),
        format!("{:x}", Sha256::digest(&body))
    );
    let selected = parsed.image("9007199254740995")?;
    assert_eq!(selected.edit_id, "9007199254740993");
    assert_eq!(selected.comment, "Original <&> artwork\nSecond line");
    assert!(selected.front);
    assert!(!selected.provider_approved);
    assert_eq!(
        selected.source,
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995.png")
    );
    assert_eq!(
        selected.request_source,
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995")
    );
    assert_eq!(parsed.images()[1].id, "9007199254740997");
    assert_eq!(parsed.images()[1].types[2], "A future type");
    assert!(!parsed.images()[1].front);
    assert!(parsed.images()[1].provider_approved);
    assert!(parsed.image("not-listed").is_err());
    let serialized = serde_json::to_value(parsed)?;
    assert!(serialized["images"][0].get("approved_at").is_none());
    assert!(serialized["images"][0].get("rights").is_none());
    Ok(())
}

#[test]
fn caa_listing_never_substitutes_a_release_group_other_release_or_source_authority() -> TestResult {
    let body = serde_json::to_vec(&listing())?;
    assert!(CaaListing::parse(OTHER, &body).is_err());
    assert!(CaaListing::parse("not-a-release", &body).is_err());
    for source in [
        format!("https://musicbrainz.org/release-group/{RELEASE}"),
        format!("https://musicbrainz.org/release/{OTHER}"),
        format!("https://musicbrainz.org/release/{RELEASE}?x=1"),
        format!("https://musicbrainz.org@untrusted.example/release/{RELEASE}"),
        format!("https://untrusted.example/release/{RELEASE}"),
    ] {
        let mut raw = listing();
        raw["release"] = json!(source);
        assert!(parse(&raw).is_err(), "{source}");
    }
    for source in [
        format!("https://coverartarchive.org/release/{OTHER}/9007199254740995.png"),
        format!("https://coverartarchive.org/release/{RELEASE}/123.png"),
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995-1200.jpg"),
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995.png?x=1"),
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995.png#fragment"),
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995.png/extra"),
        format!("https://coverartarchive.org/release/{RELEASE}/9007199254740995../x"),
        format!("https://coverartarchive.org:444/release/{RELEASE}/9007199254740995.png"),
        format!(
            "https://coverartarchive.org@untrusted.example/release/{RELEASE}/9007199254740995.png"
        ),
        "file:///private.png".into(),
        "https://127.0.0.1/private.png".into(),
        "https://untrusted.example/private.png".into(),
    ] {
        let mut raw = listing();
        raw["images"][0]["image"] = json!(source);
        assert!(parse(&raw).is_err(), "{source}");
    }
    Ok(())
}

#[test]
fn caa_identifiers_are_lossless_positive_decimal_and_never_ambiguous() -> TestResult {
    let mut raw = listing();
    raw["images"][0]["id"] = json!(9007199254740995u64);
    assert_eq!(parse(&raw)?.images()[0].id, "9007199254740995");
    for value in [
        json!(null),
        json!(true),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(""),
        json!("0"),
        json!("01"),
        json!("+1"),
        json!("1 "),
        json!("../escape"),
        json!("123456789012345678901"),
    ] {
        for field in ["id", "edit"] {
            let mut raw = listing();
            raw["images"][0][field] = value.clone();
            assert!(parse(&raw).is_err(), "{field}: {value}");
        }
    }
    let mut raw = listing();
    raw["images"]
        .as_array_mut()
        .ok_or("missing images")?
        .push(image("9007199254740995"));
    assert!(parse(&raw).is_err());
    Ok(())
}

#[test]
fn caa_listing_requires_metadata_without_inventing_missing_flags_or_types() -> TestResult {
    for field in [
        "id", "edit", "image", "types", "front", "back", "approved", "comment",
    ] {
        let mut raw = listing();
        raw["images"][0]
            .as_object_mut()
            .ok_or("missing image object")?
            .remove(field);
        assert!(parse(&raw).is_err(), "{field}");
    }
    for (field, value) in [
        ("types", json!(["Front", "Front"])),
        ("types", json!([""])),
        ("types", json!(["Back"])),
        ("types", json!(["Front", "\u{0}"])),
        ("types", json!([null])),
        ("front", json!(null)),
        ("back", json!(true)),
        ("approved", json!("true")),
        ("comment", json!("invalid\u{0}comment")),
    ] {
        let mut raw = listing();
        raw["images"][0][field] = value;
        assert!(parse(&raw).is_err(), "{field}");
    }
    let mut raw = listing();
    raw["images"][0]["front"] = json!(false);
    raw["images"][0]["types"] = json!([]);
    raw["images"][0]["comment"] = json!("");
    assert!(parse(&raw)?.images()[0].types.is_empty());
    raw["images"] = json!([]);
    assert!(parse(&raw)?.images().is_empty());
    Ok(())
}

#[test]
fn caa_body_image_count_and_text_limits_are_exact_and_do_not_truncate() -> TestResult {
    let mut raw = listing();
    raw["images"][0]["comment"] = json!("x".repeat(4096));
    parse(&raw)?;
    raw["images"][0]["comment"] = json!("x".repeat(4097));
    assert!(parse(&raw).is_err());
    let mut raw = listing();
    raw["images"][0]["types"] = json!(["Front", "x".repeat(128)]);
    parse(&raw)?;
    raw["images"][0]["types"] = json!(["Front", "x".repeat(129)]);
    assert!(parse(&raw).is_err());
    let mut raw = listing();
    let types = (0..31)
        .map(|n| format!("Type{n}"))
        .chain(["Front".into()])
        .collect::<Vec<_>>();
    raw["images"][0]["types"] = json!(types);
    parse(&raw)?;
    raw["images"][0]["types"]
        .as_array_mut()
        .ok_or("missing types")?
        .push(json!("Extra"));
    assert!(parse(&raw).is_err());
    let mut raw = listing();
    raw["images"] = json!(
        (1..=MAX_CAA_IMAGES)
            .map(|id| image(&id.to_string()))
            .collect::<Vec<_>>()
    );
    assert_eq!(parse(&raw)?.images().len(), MAX_CAA_IMAGES);
    raw["images"]
        .as_array_mut()
        .ok_or("missing images")?
        .push(image("999999"));
    assert!(parse(&raw).is_err());
    let mut body = serde_json::to_vec(&listing())?;
    body.resize(MAX_CAA_LISTING, b' ');
    CaaListing::parse(RELEASE, &body)?;
    body.push(b' ');
    assert!(CaaListing::parse(RELEASE, &body).is_err());
    assert!(CaaListing::parse(RELEASE, b"").is_err());
    assert!(CaaListing::parse(RELEASE, b"{").is_err());
    let mut body = serde_json::to_vec(&listing())?;
    body.extend_from_slice(b"{}");
    assert!(CaaListing::parse(RELEASE, &body).is_err());
    Ok(())
}

#[test]
fn caa_redirects_keep_the_exact_archive_item_file_https_and_narrow_authorities() -> TestResult {
    let request = CaaRequest::listing(RELEASE)?;
    let archive = format!("https://archive.org/download/mbid-{RELEASE}/index.json");
    let shard = format!("https://ia800504.us.archive.org/9/items/mbid-{RELEASE}/index.json");
    assert_eq!(request.follow(&archive)?.follow(&shard)?.source(), shard);
    request.follow(&format!(
        "https://s3.us.archive.org/mbid-{RELEASE}/index.json"
    ))?;
    for source in [
        format!("http://archive.org/download/mbid-{RELEASE}/index.json"),
        format!("https://archive.org:444/download/mbid-{RELEASE}/index.json"),
        format!("https://archive.org/download/mbid-{OTHER}/index.json"),
        format!("https://archive.org/download/mbid-{RELEASE}/different.json"),
        format!("{archive}?x=1"),
        format!("{archive}#fragment"),
        format!("https://archive.org@untrusted.example/download/mbid-{RELEASE}/index.json"),
        format!("https://archive.org.untrusted.example/download/mbid-{RELEASE}/index.json"),
        format!("https://untrusted.us.archive.org/9/items/mbid-{RELEASE}/index.json"),
        format!("https://ia800504.us.archive.org/9/items/mbid-{OTHER}/index.json"),
        format!("https://ia800504.us.archive.org/9/items/mbid-{RELEASE}/index.json/extra"),
        "https://127.0.0.1/private".into(),
        "file:///private".into(),
        "https://untrusted.example/private".into(),
    ] {
        assert!(request.follow(&source).is_err(), "{source}");
    }
    let listing = parse(&listing())?;
    let image = CaaRequest::image(&listing, "9007199254740995")?;
    let expected =
        format!("https://archive.org/download/mbid-{RELEASE}/mbid-{RELEASE}-9007199254740995.png");
    image.follow(&expected)?;
    assert!(image.follow(&archive).is_err());
    assert!(request.follow(&expected).is_err());
    assert!(CaaRequest::image(&listing, "123").is_err());
    assert!(
        image
            .follow(&expected.replace(".png", "-1200.jpg"))
            .is_err()
    );
    assert!(image.follow(&expected.replace(".png", ".jpg")).is_err());
    Ok(())
}

#[test]
fn caa_redirects_reject_cycles_and_the_fifth_hop() -> TestResult {
    let original = CaaRequest::listing(RELEASE)?;
    let mut request = original.clone();
    for number in 0..4 {
        request = request.follow(&format!(
            "https://ia800504.us.archive.org/{number}/items/mbid-{RELEASE}/index.json"
        ))?;
    }
    assert_eq!(request.sources().len(), 5);
    assert!(
        request
            .follow(&format!(
                "https://archive.org/download/mbid-{RELEASE}/index.json"
            ))
            .is_err()
    );
    let archive = format!("https://archive.org/download/mbid-{RELEASE}/index.json");
    assert!(original.follow(&archive)?.follow(&archive).is_err());
    assert!(original.follow(&"x".repeat(4097)).is_err());
    Ok(())
}
