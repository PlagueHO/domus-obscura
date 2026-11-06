const RELEASE: &str = "20000000-0000-4000-8000-000000000001";
const GROUP: &str = "30000000-0000-4000-8000-000000000001";
const ARTIST: &str = "10000000-0000-4000-8000-000000000001";
const GUEST: &str = "10000000-0000-4000-8000-000000000002";
const TITLE: &str = "Title / <Three>: \u{1f3b5}";
const ALBUM: &str = "Album: \u{e9} / preserved";
fn release_body(date: Option<&str>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let credit = json!([
        {"artist":{"id":ARTIST,"name":"Canonical"},"name":"Credited","joinphrase":" feat. "},
        {"artist":{"id":GUEST,"name":"Guest"},"name":"Guest","joinphrase":""}
    ]);
    let track = |index: u32, position: u16| {
        json!({
            "id":format!("40000000-0000-4000-8000-{index:012x}"),
            "position":position,"number":format!("source-{position}"),
            "title":if index == 3 { TITLE } else { "Another generated track" },
            "artist-credit":credit.clone(),
            "recording":{"id":format!("50000000-0000-4000-8000-{index:012x}"),"video":false}
        })
    };
    Ok(serde_json::to_vec(&json!({
        "id":RELEASE,"title":ALBUM,"date":date,"artist-credit":credit,
        "release-group":{"id":GROUP},
        "media":[
            {"position":1,"track-count":2,"tracks":[track(1,1),track(2,2)]},
            {"position":2,"track-count":1,"tracks":[track(3,1)]}
        ]
    }))?)
}

fn cover() -> Result<CoverJpeg, Box<dyn std::error::Error>> {
    Ok(normalize_cover(&cover_source()?, None)?)
}
