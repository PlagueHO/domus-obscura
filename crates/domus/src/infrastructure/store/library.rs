use super::{ScanClaim, Store};
use crate::domain::library::{
    LibraryAlbum, LibraryArtist, LibraryCoverage, LibraryFile, LibraryMatch, LibraryMatchQuery,
    LibraryMatches, LibraryMetadata, library_key,
};
use crate::domain::{LibraryPath, Page};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn save_metadata(
    connection: &Connection,
    claim: &ScanClaim,
    path: &LibraryPath,
    metadata: &LibraryMetadata,
) -> Result<()> {
    let artist = if metadata.tags.album_artists.is_empty() {
        &metadata.tags.artists
    } else {
        &metadata.tags.album_artists
    };
    let name = artist.join("; ");
    let artist_id = if metadata.status == "tagged" && !name.trim().is_empty() {
        connection.execute(
            "INSERT INTO library_artists(location_id,name_key,name) VALUES (?1,?2,?3)
             ON CONFLICT(location_id,name_key) DO NOTHING",
            params![claim.location, library_key(&name), name],
        )?;
        Some(connection.query_row(
            "SELECT id FROM library_artists WHERE location_id=?1 AND name_key=?2",
            params![claim.location, library_key(&name)],
            |row| row.get::<_, i64>(0),
        )?)
    } else {
        None
    };
    let album_id = if let (Some(artist), Some(album)) = (artist_id, &metadata.tags.album) {
        connection.execute(
            "INSERT INTO library_albums(location_id,artist_id,title_key,title,folder)
             VALUES (?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",
            params![
                claim.location,
                artist,
                library_key(album),
                album,
                path.parent()
            ],
        )?;
        Some(connection.query_row(
            "SELECT id FROM library_albums
             WHERE location_id=?1 AND artist_id=?2 AND title_key=?3 AND folder=?4",
            params![claim.location, artist, library_key(album), path.parent()],
            |row| row.get::<_, i64>(0),
        )?)
    } else {
        None
    };
    connection.execute(
        "INSERT INTO library_metadata(entry_id,location_id,scan_id,body,status,active,artist_id,album_id)
         SELECT id,location_id,scan_id,?4,?5,1,?6,?7 FROM entries
         WHERE location_id=?1 AND relative_path=?2 AND scan_id=?3 AND kind='file' AND present=1
         ON CONFLICT(entry_id) DO UPDATE SET scan_id=excluded.scan_id,body=excluded.body,
         status=excluded.status,active=1,artist_id=excluded.artist_id,album_id=excluded.album_id,
         observed_at=unixepoch(),profile=1",
        params![claim.location, path.as_str(), claim.id, serde_json::to_string(metadata)?,
            metadata.status, artist_id, album_id],
    )?;
    Ok(())
}

fn album_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryAlbum> {
    Ok(LibraryAlbum {
        id: row.get(0)?,
        artist_id: row.get(1)?,
        artist: row.get(2)?,
        title: row.get(3)?,
        folder: row.get(4)?,
        file_count: row.get(5)?,
    })
}

fn file_row(row: &rusqlite::Row<'_>) -> Result<LibraryFile> {
    let body: Option<String> = row.get(6)?;
    let mut metadata = match body {
        Some(body) => serde_json::from_str::<LibraryMetadata>(&body)?,
        None => LibraryMetadata {
            status: "unobserved".into(),
            ..Default::default()
        },
    };
    let active: Option<bool> = row.get(7)?;
    if active == Some(false) {
        metadata = LibraryMetadata {
            status: "stale".into(),
            error: Some("saved metadata no longer matches the current entry observation".into()),
            ..Default::default()
        };
    }
    Ok(LibraryFile {
        id: row.get(0)?,
        relative_path: row.get(1)?,
        present: row.get(2)?,
        scan_id: row.get(3)?,
        observed_at: row.get(4)?,
        metadata: LibraryMetadata {
            size: row.get(5)?,
            ..metadata
        },
    })
}

const FILE_COLUMNS: &str =
    "e.id,e.relative_path,e.present,e.scan_id,m.observed_at,e.size,m.body,m.active";
const ALBUM_COLUMNS: &str = "a.id,a.artist_id,r.name,a.title,a.folder,a.file_count";

impl Store {
    pub fn library_artists(&self, location: i64, page: Page) -> Result<Vec<LibraryArtist>> {
        self.location(location)?;
        Ok(self
            .connection
            .prepare_cached(
                "SELECT id,name,file_count FROM library_artists
             WHERE location_id=?1 AND file_count>0 AND id>?2 ORDER BY id LIMIT ?3",
            )?
            .query_map(params![location, page.after(), page.limit()], |row| {
                Ok(LibraryArtist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    file_count: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn library_albums(
        &self,
        location: i64,
        artist: Option<i64>,
        page: Page,
    ) -> Result<Vec<LibraryAlbum>> {
        self.location(location)?;
        if let Some(artist) = artist {
            self.connection.query_row(
                "SELECT id FROM library_artists WHERE location_id=?1 AND id=?2",
                params![location, artist],
                |row| row.get::<_, i64>(0),
            )?;
        }
        let filter = if artist.is_some() {
            " AND a.artist_id=?4"
        } else {
            " AND ?4 IS NULL"
        };
        Ok(self
            .connection
            .prepare_cached(&format!(
            "SELECT {ALBUM_COLUMNS} FROM library_albums a JOIN library_artists r ON r.id=a.artist_id
             WHERE a.location_id=?1 AND a.file_count>0 AND a.id>?2{filter} ORDER BY a.id LIMIT ?3"
        ))?
            .query_map(
                params![location, page.after(), page.limit(), artist],
                album_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn library_album(&self, location: i64, album: i64) -> Result<LibraryAlbum> {
        Ok(self.connection.query_row(
            &format!(
                "SELECT {ALBUM_COLUMNS} FROM library_albums a JOIN library_artists r
              ON r.id=a.artist_id WHERE a.location_id=?1 AND a.id=?2 AND a.file_count>0"
            ),
            params![location, album],
            album_row,
        )?)
    }

    pub fn library_file(&self, location: i64, entry: i64) -> Result<LibraryFile> {
        self.connection.query_row(
            &format!("SELECT {FILE_COLUMNS} FROM entries e LEFT JOIN library_metadata m ON m.entry_id=e.id
              WHERE e.location_id=?1 AND e.id=?2 AND e.kind='file'"),
            params![location,entry],
            |row| Ok(file_row(row)),
        )?
    }

    pub fn library_album_files(
        &self,
        location: i64,
        album: i64,
        page: Page,
    ) -> Result<Vec<LibraryFile>> {
        self.library_album(location, album)?;
        self.connection
            .prepare_cached(&format!(
                "SELECT {FILE_COLUMNS} FROM library_metadata m JOIN entries e ON e.id=m.entry_id
             WHERE m.location_id=?1 AND m.album_id=?2 AND m.active=1 AND m.entry_id>?3
             ORDER BY m.entry_id LIMIT ?4"
            ))?
            .query_map(
                params![location, album, page.after(), page.limit()],
                |row| Ok(file_row(row)),
            )?
            .map(|row| row.map_err(Error::from).and_then(|row| row))
            .collect()
    }

    /// Assets are saved direct-folder observations, not arbitrary file downloads.
    pub fn library_album_assets(
        &self,
        location: i64,
        album: i64,
        page: Page,
    ) -> Result<Vec<LibraryFile>> {
        let album = self.library_album(location, album)?;
        self.connection.prepare_cached(
            &format!("SELECT {FILE_COLUMNS} FROM entries e LEFT JOIN library_metadata m ON m.entry_id=e.id
             WHERE e.location_id=?1 AND e.parent_path=?2 AND e.kind='file' AND e.present=1 AND e.id>?3
             AND (m.status IN ('asset','unsupported') OR m.entry_id IS NULL)
             ORDER BY e.id LIMIT ?4"),
        )?.query_map(params![location,album.folder,page.after(),page.limit()], |row| Ok(file_row(row)))?
            .map(|row| row.map_err(Error::from).and_then(|row| row)).collect()
    }

    pub fn library_coverage(&self, location: i64, folder: &LibraryPath) -> Result<LibraryCoverage> {
        let root = self.location(location)?;
        let (files, issues, unobserved) = self.connection.query_row(
            "SELECT files,issues,total_files-files FROM library_metadata_counts WHERE location_id=?1",
            [location], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let folder_state = self.connection.query_row(
            "SELECT state,observed_at,last_error FROM library_folder_observations WHERE location_id=?1 AND folder=?2",
            params![location,folder.as_str()],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional()?;
        let (folder_state, folder_observed_at, folder_error) = folder_state
            .map_or((None, None, None), |(state, time, error)| {
                (Some(state), time, error)
            });
        Ok(LibraryCoverage {
            location_id: location,
            coverage: root.coverage,
            observed_at: root.observed_at,
            last_error: root.last_error,
            metadata_files: files,
            metadata_issues: issues,
            unobserved_files: unobserved,
            folder_state,
            folder_observed_at,
            folder_error,
        })
    }

    pub fn library_matches(
        &self,
        location: i64,
        queries: &[LibraryMatchQuery],
    ) -> Result<LibraryMatches> {
        if queries.is_empty()
            || queries.len() > 24
            || queries.iter().any(|q| {
                q.artist.trim().is_empty()
                    || q.artist.len() > 4096
                    || q.album.as_ref().is_some_and(|s| s.len() > 4096)
            })
        {
            return Err(Error::InvalidInput(
                "local matching requires 1 through 24 bounded artist/album queries".into(),
            ));
        }
        let coverage = self.library_coverage(location, &LibraryPath::parse("")?)?;
        let mut matches = Vec::with_capacity(queries.len());
        for (index, query) in queries.iter().enumerate() {
            let artist = self.connection.query_row(
                "SELECT id FROM library_artists WHERE location_id=?1 AND name_key=?2 AND file_count>0",
                params![location,library_key(&query.artist)], |row| row.get::<_,i64>(0),
            ).optional()?;
            let mut albums = Vec::new();
            if let (Some(artist), Some(title)) = (artist, &query.album) {
                albums = self
                    .connection
                    .prepare_cached(&format!(
                        "SELECT {ALBUM_COLUMNS} FROM library_albums a JOIN library_artists r
                     ON r.id=a.artist_id WHERE a.location_id=?1 AND a.artist_id=?2
                     AND a.title_key=?3 AND a.file_count>0 ORDER BY a.id LIMIT 50"
                    ))?
                    .query_map(params![location, artist, library_key(title)], album_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
            }
            matches.push(LibraryMatch {
                query_index: index,
                artist_id: artist,
                albums,
            });
        }
        Ok(LibraryMatches { coverage, matches })
    }
}
