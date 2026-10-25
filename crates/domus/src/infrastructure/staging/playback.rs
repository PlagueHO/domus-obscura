use super::*;
use crate::domain::{LibraryPath, OutputPreview};
use std::collections::HashSet;

fn create_directory(parent: &Dir, name: &str, permissions: &FilePermissions) -> Result<Dir> {
    let mut builder = DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use cap_std::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    parent.create_dir_with(name, &builder)?;
    let child = parent.open_dir_nofollow(name)?;
    check_directory(&child, permissions)?;
    sync_directory(parent)?;
    Ok(child)
}

impl StagingRoot {
    /// Create a separate playback test tree, never a configured destination.
    /// Existing or partial exports are retained and cannot be overwritten.
    pub(crate) fn export_playback(
        &self,
        run: &NormalizationRun,
        work: &[NormalizationWork],
        outputs: &[OutputPreview],
        cover: &CoverJpeg,
        heartbeat: &dyn Fn() -> std::io::Result<()>,
    ) -> Result<PathBuf> {
        let folder = outputs
            .first()
            .ok_or_else(conflict)?
            .relative_folder
            .clone();
        LibraryPath::parse(&folder)?;
        let mut names = HashSet::new();
        if folder.is_empty()
            || run.completed_at.is_none()
            || work.len() != usize::from(run.files)
            || work.len() != outputs.len()
            || work
                .iter()
                .zip(outputs)
                .enumerate()
                .any(|(index, (item, output))| {
                    item.track_number as usize != index + 1
                        || output.relative_folder != folder
                        || output.filename.contains('/')
                        || output.filename.eq_ignore_ascii_case("folder.jpg")
                        || !names.insert(output.filename.to_lowercase())
                })
        {
            return Err(conflict());
        }
        for output in outputs {
            if output.filename.is_empty() {
                return Err(conflict());
            }
            LibraryPath::parse(&output.filename)?;
        }
        let normalized = self.open_normalization(run)?;
        normalized.verify_cover()?;
        if hex(cover.sha256()) != run.cover_sha256 || cover.bytes().len() as u64 != run.cover_bytes
        {
            return Err(conflict());
        }
        // Verify all source outputs before creating a test export directory.
        for item in work {
            normalized.verify_audio_with_progress(item, heartbeat)?;
        }
        heartbeat()?;
        let export_name = format!("playback-{id}", id = run.id);
        let export = create_directory(&self.directory, &export_name, &self.permissions)?;
        let mut album = export.try_clone()?;
        let mut path = self.path.join(&export_name);
        for component in folder.split('/') {
            heartbeat()?;
            album = create_directory(&album, component, &self.permissions)?;
            path.push(component);
        }
        for (item, output) in work.iter().zip(outputs) {
            heartbeat()?;
            let evidence = item.receipt.as_ref().ok_or_else(conflict)?;
            let mut source =
                open_file(&normalized.directory, &item.staging_name, &self.permissions)?;
            let mut destination = create_file(&album, &output.filename, &self.permissions)?;
            let mut remaining = evidence.bytes;
            let mut buffer = [0; 65_536];
            while remaining > 0 {
                heartbeat()?;
                let count = remaining.min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..count])?;
                heartbeat()?;
                destination.write_all(&buffer[..count])?;
                remaining -= count as u64;
            }
            if source.read(&mut [0; 1])? != 0 {
                return Err(conflict());
            }
            heartbeat()?;
            destination.sync_all()?;
            if hex(&hash_file_with_progress(
                &mut destination,
                evidence.bytes,
                heartbeat,
            )?) != evidence.sha256
            {
                return Err(conflict());
            }
        }
        heartbeat()?;
        let mut image = create_file(&album, "folder.jpg", &self.permissions)?;
        image.write_all(cover.bytes())?;
        image.sync_all()?;
        if hex(&hash_file_with_progress(
            &mut image,
            run.cover_bytes,
            heartbeat,
        )?) != run.cover_sha256
        {
            return Err(conflict());
        }
        sync_directory(&album)?;
        heartbeat()?;
        // This record is outside the album folder and appears only after all
        // copies are synced and read back under the live worker claim.
        write_record(
            &export,
            "complete.json",
            &serde_json::to_vec(&serde_json::json!({
                "purpose":"playback-test",
                "run_id":run.id,
                "job_id":run.job_id,
                "plan_version":run.plan_version,
                "recipe_sha256":run.recipe_sha256,
                "files":run.files,
                "relative_folder":folder
            }))?,
            &self.permissions,
        )?;
        heartbeat()?;
        Ok(path)
    }
}
