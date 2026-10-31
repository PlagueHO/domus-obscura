CREATE TABLE artist_photo_bindings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    artist_provider TEXT NOT NULL,
    artist_id TEXT NOT NULL,
    image_provider TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    photo TEXT,
    UNIQUE(artist_provider, artist_id, image_provider)
) STRICT;

-- A removed association keeps its version so an old browser cannot recreate it.
CREATE TABLE commons_gate (
    id INTEGER PRIMARY KEY CHECK(id=1),
    generation INTEGER NOT NULL DEFAULT 0,
    pending INTEGER NOT NULL DEFAULT 0 CHECK(pending IN (0,1)),
    delay_seconds INTEGER NOT NULL DEFAULT 0 CHECK(delay_seconds BETWEEN 0 AND 4294967295)
) STRICT;
INSERT INTO commons_gate(id) VALUES(1);
