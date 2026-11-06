use super::{RELEASE, TestResult, cover, cover_source, flac, mp3, release_body};
use domus::application::{add_location, preview_album_replacements, scan_location};
use domus::domain::{AudioFormat, JobState, OutputSettings, Page, ReplacementAction};
use domus::infrastructure::artwork::CoverJpeg;
use domus::infrastructure::catalog::{CatalogQuery, CatalogResult};
use domus::infrastructure::media::{NormalizedAudio, normalize_audio_with_profile};
use domus::infrastructure::staging::StagingRoot;
use domus::infrastructure::store::{
    AlbumReplacementDecision, JobClaim, NormalizationWork, PlanRequest, SourceFile, Store,
};
use private::tempdir;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::PathBuf;
use tempfile::TempDir;

struct Fixture {
    store: Store,
    claim: JobClaim,
    job: i64,
    version: i64,
    main: i64,
    database: PathBuf,
    source: Vec<u8>,
    cover: CoverJpeg,
    staging: Option<StagingRoot>,
    scratch: TempDir,
    _state: TempDir,
    _library: TempDir,
}

#[derive(Default)]
struct FixtureInputs {
    profile: Option<domus::domain::AudioProfile>,
    directories: Option<[TempDir; 3]>,
    source: Option<Vec<u8>>,
    defer_acquisition: bool,
}

#[allow(dead_code)]
impl Fixture {
    fn new(format: AudioFormat) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(format, false)
    }

    fn build(format: AudioFormat, staged: bool) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_inner(format, staged, true)
    }

    fn build_inner(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_receipts(format, staged, approve_cover, true)
    }

    fn build_with_receipts(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_backup(format, staged, approve_cover, receive, None)
    }

    fn build_with_backup(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
        backup: Option<&std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_options(format, staged, approve_cover, receive, backup, None)
    }

    fn build_with_replacement(
        format: AudioFormat,
        staged: bool,
        action: ReplacementAction,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_options(format, staged, true, true, None, Some(action))
    }

    fn build_with_backup_replacement(
        format: AudioFormat,
        staged: bool,
        backup: &std::path::Path,
        action: ReplacementAction,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_options(format, staged, true, true, Some(backup), Some(action))
    }

    fn build_with_options(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
        backup: Option<&std::path::Path>,
        replacement_action: Option<ReplacementAction>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_profile(
            format,
            staged,
            approve_cover,
            receive,
            backup,
            replacement_action,
            None,
        )
    }

    fn build_with_profile(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
        backup: Option<&std::path::Path>,
        replacement_action: Option<ReplacementAction>,
        profile: Option<domus::domain::AudioProfile>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build_with_inputs(
            format,
            staged,
            approve_cover,
            receive,
            backup,
            replacement_action,
            FixtureInputs {
                profile,
                ..Default::default()
            },
        )
    }

    fn build_with_inputs(
        format: AudioFormat,
        staged: bool,
        approve_cover: bool,
        receive: bool,
        backup: Option<&std::path::Path>,
        replacement_action: Option<ReplacementAction>,
        inputs: FixtureInputs,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let [state, library, scratch] = match inputs.directories {
            Some(directories) => directories,
            None => [tempdir()?, tempdir()?, tempdir()?],
        };
        let profile = inputs.profile;
        let database = state.path().join("state.db");
        let mut store = Store::open(&database)?;
        let main = add_location(&store, library.path(), "main")?;
        scan_location(&mut store, main)?;
        let mut locations = vec![main];
        if let Some(backup) = backup {
            let id = add_location(&store, backup, "backup")?;
            scan_location(&mut store, id)?;
            locations.push(id);
        }
        let settings = if let Some(profile) = profile {
            OutputSettings {
                format,
                accepted_formats: Some(vec![format]),
                audio_policy: Some(domus::domain::AudioPolicy {
                    accepted: vec![profile],
                    preferred: profile,
                    ..domus::domain::AudioPolicy::default()
                }),
                ..OutputSettings::default()
            }
        } else {
            OutputSettings::default()
        };
        store.save_settings(None, &settings)?;
        let job = store.enqueue(1)?;
        let body = release_body(Some("2024-02"))?;
        let query = CatalogQuery::release(RELEASE)?;
        let raw = Connection::open(&database)?;
        let now: i64 = raw.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
        let payload_sha256 = Sha256::digest(&body).to_vec();
        raw.execute(
            "INSERT INTO catalog_cache(source,body,payload_sha256,fetched_at,expires_at,accessed_at_ms)
             VALUES (?1,?2,?3,?4,?4+86400,?4*1000)",
            params![query.source(), body, payload_sha256, now],
        )?;
        let catalog = CatalogResult {
            source: query.source().into(),
            fetched_at: now,
            cached: true,
            payload_sha256,
            data: query.parse_release(&body)?,
        };
        let plan = store.prepare_catalog_plan(
            PlanRequest {
                audio_profile: profile,
                job_id: job,
                expected_version: 1,
                settings_version: 1,
                format: Some(format),
                edition: catalog.data.selection(None)?,
                locations,
                accepts_incomplete_coverage: false,
            },
            catalog,
        )?;
        if approve_cover {
            let candidate =
                store.prepare_artwork(job, plan.version, "123456789", &cover_source()?, None)?;
            store.approve_artwork(job, plan.version, candidate.id, &candidate.sha256)?;
        }
        store.approve_plan(job, plan.version)?;
        if let Some(action) = replacement_action {
            for target in &plan.plan.destinations {
                let folder = std::path::Path::new(&target.root).join(
                    plan.plan.outputs[0]
                        .relative_folder
                        .replace('/', std::path::MAIN_SEPARATOR_STR),
                );
                std::fs::create_dir_all(&folder)?;
                std::fs::write(folder.join("folder.jpg"), b"old cover")?;
                std::fs::write(folder.join("manual.txt"), b"user extra")?;
            }
            let previews = preview_album_replacements(&store, job, plan.version)?;
            for target in &plan.plan.destinations {
                let preview = previews
                    .iter()
                    .find(|preview| preview.location_id == target.location_id)
                    .ok_or("replacement preview")?;
                store.record_album_replacement_decision(
                    preview,
                    AlbumReplacementDecision {
                        action,
                        expected_manifest_sha256: preview.manifest_sha256.clone(),
                    },
                )?;
            }
        }
        let claim = store
            .claim_approved_plan(job, plan.version, "worker", 300)?
            .ok_or("missing claim")?
            .claim;
        store.advance(&claim, JobState::Queued, JobState::Searching)?;
        let source = if let Some(source) = inputs.source {
            source
        } else if let Some(profile) = profile.filter(|profile| profile.format() == AudioFormat::Mp3)
        {
            super::profiles::frames(
                &profile
                    .bitrate()
                    .map_or_else(|| vec![32, 192, 320], |bitrate| vec![bitrate; 3]),
                false,
                false,
                0,
                false,
            )
        } else {
            match format {
                AudioFormat::Mp3 => mp3()?,
                AudioFormat::Flac => flac(),
            }
        };
        let files = (1..=3)
            .map(|track| SourceFile {
                remote_path: format!("Peer\\Album\\track-{track}.{}", format.extension()),
                expected_bytes: source.len() as i64,
            })
            .collect::<Vec<_>>();
        if !inputs.defer_acquisition {
            store.begin_acquisition(&claim, "generated-peer", &files)?;
        }
        let staging = if staged {
            Some(StagingRoot::open(&mut store, scratch.path())?)
        } else {
            None
        };
        for file in store.transfers(job, plan.version, Page::new(0, 200)?)? {
            store.start_transfer(&claim, file.id)?;
            store.accept_transfer(&claim, file.id, file.track_number.into())?;
            if !receive {
                continue;
            }
            if let Some(staging) = &staging {
                let active = store
                    .transfers(job, plan.version, Page::new(file.id - 1, 1)?)?
                    .pop()
                    .ok_or("missing active transfer")?;
                let received = staging.receive(&active, &mut Cursor::new(&source))?;
                store.receive_transfer(
                    &claim,
                    file.id,
                    received.byte_length() as i64,
                    received.sha256(),
                )?;
            } else {
                store.receive_transfer(
                    &claim,
                    file.id,
                    source.len() as i64,
                    &Sha256::digest(&source).into(),
                )?;
            }
        }
        if receive && !inputs.defer_acquisition {
            store.advance(&claim, JobState::Downloading, JobState::Validating)?;
        }
        Ok(Self {
            store,
            claim,
            job,
            version: plan.version,
            main,
            database,
            source,
            cover: cover()?,
            staging,
            scratch,
            _state: state,
            _library: library,
        })
    }

    fn proof(
        &self,
        work: &NormalizationWork,
    ) -> Result<NormalizedAudio, Box<dyn std::error::Error>> {
        Ok(normalize_audio_with_profile(
            &mut Cursor::new(&self.source),
            &mut Cursor::new(Vec::new()),
            work.format,
            work.audio,
            &work.tags,
            &self.cover,
        )?)
    }

    fn bind_roots(&self) -> TestResult {
        use domus::infrastructure::filesystem::native_root_identity;
        self.store
            .bind_location(self.main, &native_root_identity(self._library.path())?)?;
        if self.staging.is_some() {
            self.store.bind_staging_root(
                self.scratch.path(),
                &native_root_identity(self.scratch.path())?,
            )?;
        }
        Ok(())
    }
}
