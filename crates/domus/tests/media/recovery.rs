use super::*;
use domus::application::recovery::{continue_recovery, delete_recovery, preview_recovery};
use domus::infrastructure::store::{RecoveryDecision, RecoveryEntry};

fn retained(fixture: &Fixture) -> Result<RecoveryEntry, Box<dyn std::error::Error>> {
    fixture
        .store
        .recovery_inventory(Page::new(0, 100)?, Some(fixture.job))?
        .into_iter()
        .find(|e| e.category == "retained")
        .ok_or("retained entry".into())
}

fn deletion(
    fixture: &mut Fixture,
    id: i64,
) -> Result<RecoveryDecision, Box<dyn std::error::Error>> {
    let preview = preview_recovery(&mut fixture.store, id)?;
    assert!(preview.verified, "{:?}", preview.unavailable_reason);
    Ok(RecoveryDecision {
        expected_preview_sha256: preview.preview_sha256.ok_or("preview")?,
        acknowledge_irreversible_deletion: true,
    })
}

#[test]
fn recovery_cleanup_removes_only_completed_receipted_scratch_and_keeps_evidence() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let unrelated = fixture.scratch.path().join("unrelated.txt");
    std::fs::write(&unrelated, b"not job-owned")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    let operation = fixture
        .store
        .recovery_operation(run * 8 + 1)?
        .ok_or("cleanup operation")?;
    assert_eq!(operation.state, "complete", "{:?}", operation.last_error);
    assert!(
        !fixture
            .scratch
            .path()
            .join(format!("normalization-{run}"))
            .exists()
    );
    assert_eq!(std::fs::read(unrelated)?, b"not job-owned");
    assert!(fixture.scratch.path().join("owner.lock").is_file());
    assert_eq!(std::fs::read_dir(fixture.scratch.path())?.count(), 2);
    let raw = Connection::open(&fixture.database)?;
    assert_eq!(
        raw.query_row("SELECT count(*) FROM normalization_receipts", [], |r| r
            .get::<_, i64>(0))?,
        3
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM transfer_files WHERE state='received'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        3
    );
    assert!(raw.execute("DELETE FROM recovery_operations", []).is_err());
    assert!(
        raw.execute("UPDATE recovery_items SET identity='different'", [])
            .is_err()
    );
    assert_eq!(
        fixture.store.job_state(fixture.job)?.as_deref(),
        Some("complete")
    );
    Ok(())
}

#[test]
fn recovery_cleanup_preparation_failure_is_durable_and_retry_needs_the_staging_lock() -> TestResult
{
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let run = normalized(&mut fixture)?;
    let extra = fixture
        .scratch
        .path()
        .join(format!("normalization-{run}"))
        .join("unapproved.txt");
    std::fs::write(&extra, b"keep")?;
    assert_eq!(
        publish_new_album(
            &mut fixture.store,
            &fixture.claim,
            fixture.staging.as_ref().ok_or("staging")?,
            run
        )?,
        "complete"
    );
    let entry = fixture
        .store
        .recovery_inventory(Page::new(0, 100)?, Some(fixture.job))?
        .into_iter()
        .find(|e| e.id == run * 8 + 1)
        .ok_or("scratch")?;
    assert_eq!(entry.state, "pending");
    assert!(entry.last_error.is_some());
    assert!(entry.operation_id.is_none());
    assert!(continue_recovery(&mut fixture.store, entry.id, 0, true).is_err());
    std::fs::remove_file(extra)?;
    fixture.staging.take();
    let result = continue_recovery(&mut fixture.store, entry.id, 0, true)?;
    assert_eq!(result.state, "complete");
    assert_eq!(
        continue_recovery(&mut fixture.store, entry.id, result.id, true)?.state,
        "complete"
    );
    assert!(continue_recovery(&mut fixture.store, entry.id, result.id + 1, true).is_err());
    Ok(())
}

#[test]
fn recovery_deletion_is_preview_bound_preserves_visible_album_and_disables_restore() -> TestResult {
    let (mut fixture, destination) = restore_fixture()?;
    let restore = restore_decision(&mut fixture, &destination)?;
    let entry = retained(&fixture)?;
    let before = std::fs::read(album(&fixture, fixture._library.path())?.join("folder.jpg"))?;
    let mut decision = deletion(&mut fixture, entry.id)?;
    decision.acknowledge_irreversible_deletion = false;
    assert!(delete_recovery(&mut fixture.store, entry.id, decision).is_err());
    let mut stale = deletion(&mut fixture, entry.id)?;
    stale.expected_preview_sha256 = "0".repeat(64);
    assert!(delete_recovery(&mut fixture.store, entry.id, stale).is_err());
    assert!(fixture.store.recovery_operation(entry.id)?.is_none());
    let decision = deletion(&mut fixture, entry.id)?;
    let result = delete_recovery(&mut fixture.store, entry.id, decision)?;
    assert_eq!(result.state, "complete");
    assert!(!fixture._library.path().join(&entry.folder).exists());
    assert_eq!(
        std::fs::read(album(&fixture, fixture._library.path())?.join("folder.jpg"))?,
        before
    );
    assert!(
        domus::application::restore_album_replacement(
            &mut fixture.store,
            fixture.job,
            fixture.version,
            destination.location_id,
            restore
        )
        .is_err()
    );
    let preview = preview_recovery(&mut fixture.store, entry.id)?;
    assert!(!preview.verified);
    assert!(!preview.entry.eligible);
    assert_eq!(
        continue_recovery(&mut fixture.store, entry.id, result.id, false)?.state,
        "complete"
    );
    Ok(())
}

#[test]
fn recovery_deletes_the_displaced_version_after_restore_not_the_visible_original() -> TestResult {
    let (mut fixture, destination) = restore_fixture()?;
    let original = retained(&fixture)?;
    let decision = restore_decision(&mut fixture, &destination)?;
    domus::application::restore_album_replacement(
        &mut fixture.store,
        fixture.job,
        fixture.version,
        destination.location_id,
        decision,
    )?;
    let displaced = retained(&fixture)?;
    assert_ne!(original.id, displaced.id);
    assert_ne!(original.version, displaced.version);
    let current = album(&fixture, fixture._library.path())?;
    let before = std::fs::read(current.join("manual.txt"))?;
    let decision = deletion(&mut fixture, displaced.id)?;
    assert_eq!(
        delete_recovery(&mut fixture.store, displaced.id, decision)?.state,
        "complete"
    );
    assert_eq!(std::fs::read(current.join("manual.txt"))?, before);
    assert!(!fixture._library.path().join(&displaced.folder).exists());
    Ok(())
}

#[test]
fn recovery_rejects_missing_changed_extra_and_changed_visible_files_before_intent() -> TestResult {
    for changed in ["missing", "changed", "extra", "visible"] {
        let (mut fixture, _) = restore_fixture()?;
        let entry = retained(&fixture)?;
        let decision = deletion(&mut fixture, entry.id)?;
        let folder = fixture._library.path().join(&entry.folder);
        match changed {
            "missing" => std::fs::remove_file(folder.join("manual.txt"))?,
            "changed" => std::fs::write(folder.join("manual.txt"), b"different")?,
            "extra" => std::fs::write(folder.join("extra.txt"), b"unapproved")?,
            "visible" => std::fs::write(
                album(&fixture, fixture._library.path())?.join("folder.jpg"),
                b"different",
            )?,
            _ => unreachable!(),
        }
        assert!(
            delete_recovery(&mut fixture.store, entry.id, decision).is_err(),
            "{changed}"
        );
        assert!(fixture.store.recovery_operation(entry.id)?.is_none());
        assert!(folder.is_dir());
        assert!(!preview_recovery(&mut fixture.store, entry.id)?.verified);
    }
    Ok(())
}

#[test]
fn recovery_reconciles_unlink_before_receipt_and_final_status_interruption() -> TestResult {
    for boundary in ["item", "completion"] {
        let (mut fixture, _) = restore_fixture()?;
        let entry = retained(&fixture)?;
        let raw = Connection::open(&fixture.database)?;
        let trigger = if boundary == "item" {
            "CREATE TRIGGER interrupt_recovery BEFORE UPDATE OF state ON recovery_items
             WHEN NEW.state='removed' BEGIN SELECT RAISE(ABORT,'generated interruption'); END;"
        } else {
            "CREATE TRIGGER interrupt_recovery BEFORE UPDATE OF state ON recovery_operations
             WHEN NEW.state='complete' BEGIN SELECT RAISE(ABORT,'generated interruption'); END;"
        };
        raw.execute_batch(trigger)?;
        let decision = deletion(&mut fixture, entry.id)?;
        assert!(delete_recovery(&mut fixture.store, entry.id, decision).is_err());
        let operation = fixture
            .store
            .recovery_operation(entry.id)?
            .ok_or("operation")?;
        assert_eq!(operation.state, "pending");
        assert!(operation.last_error.is_some());
        raw.execute_batch("DROP TRIGGER interrupt_recovery")?;
        assert_eq!(
            continue_recovery(&mut fixture.store, entry.id, operation.id, false)?.state,
            "complete"
        );
        assert!(!fixture._library.path().join(&entry.folder).exists());
    }
    Ok(())
}

#[test]
fn recovery_interrupted_deletion_rejects_a_stale_job_plan() -> TestResult {
    let (mut fixture, _) = restore_fixture()?;
    let entry = retained(&fixture)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER interrupt_recovery BEFORE UPDATE OF state ON recovery_items
         WHEN NEW.state='removed' BEGIN SELECT RAISE(ABORT,'generated interruption'); END;",
    )?;
    let decision = deletion(&mut fixture, entry.id)?;
    assert!(delete_recovery(&mut fixture.store, entry.id, decision).is_err());
    raw.execute_batch("DROP TRIGGER interrupt_recovery")?;
    let folder = fixture._library.path().join(&entry.folder);
    let remaining = std::fs::read_dir(&folder)?
        .next()
        .ok_or("remaining file")??
        .path();
    let before = std::fs::read(&remaining)?;
    raw.execute(
        "UPDATE jobs SET plan_version=plan_version+1 WHERE id=?1",
        [fixture.job],
    )?;
    let operation = fixture
        .store
        .recovery_operation(entry.id)?
        .ok_or("operation")?;
    assert!(continue_recovery(&mut fixture.store, entry.id, operation.id, false).is_err());
    assert_eq!(std::fs::read(&remaining)?, before);
    assert_eq!(
        fixture
            .store
            .recovery_operation(entry.id)?
            .ok_or("operation")?
            .state,
        "pending"
    );
    Ok(())
}

#[test]
fn recovery_interrupted_deletion_rejects_a_changed_remaining_file() -> TestResult {
    let (mut fixture, _) = restore_fixture()?;
    let entry = retained(&fixture)?;
    let raw = Connection::open(&fixture.database)?;
    raw.execute_batch(
        "CREATE TRIGGER interrupt_recovery BEFORE UPDATE OF state ON recovery_items
         WHEN NEW.state='removed' BEGIN SELECT RAISE(ABORT,'generated interruption'); END;",
    )?;
    let decision = deletion(&mut fixture, entry.id)?;
    assert!(delete_recovery(&mut fixture.store, entry.id, decision).is_err());
    raw.execute_batch("DROP TRIGGER interrupt_recovery")?;
    let remaining = std::fs::read_dir(fixture._library.path().join(&entry.folder))?
        .next()
        .ok_or("remaining file")??
        .path();
    std::fs::write(&remaining, b"changed after interrupted deletion")?;
    let operation = fixture
        .store
        .recovery_operation(entry.id)?
        .ok_or("operation")?;
    assert!(continue_recovery(&mut fixture.store, entry.id, operation.id, false).is_err());
    assert_eq!(
        std::fs::read(remaining)?,
        b"changed after interrupted deletion"
    );
    Ok(())
}

#[test]
fn recovery_inventory_is_bounded_and_keyset_pages_have_no_duplicate_artifacts() -> TestResult {
    let (fixture, _) = restore_fixture()?;
    let expected = fixture
        .store
        .recovery_inventory(Page::new(0, 100)?, Some(fixture.job))?;
    let mut actual = Vec::new();
    let mut after = 0;
    loop {
        let page = fixture
            .store
            .recovery_inventory(Page::new(after, 1)?, Some(fixture.job))?;
        let Some(entry) = page.into_iter().next() else {
            break;
        };
        assert!(entry.id > after);
        after = entry.id;
        actual.push(entry.id);
    }
    assert_eq!(actual, expected.iter().map(|e| e.id).collect::<Vec<_>>());
    assert!(expected.iter().any(|e| e.category == "input"));
    assert!(
        expected
            .iter()
            .any(|e| e.category == "scratch" && e.state == "complete")
    );
    assert!(
        expected
            .iter()
            .any(|e| e.category == "retained" && e.eligible)
    );
    assert!(
        fixture
            .store
            .recovery_inventory(Page::new(0, 101)?, None)
            .is_err()
    );
    assert!(
        fixture
            .store
            .recovery_inventory(Page::new(0, 50)?, Some(fixture.job + 1))?
            .is_empty()
    );
    Ok(())
}

#[test]
fn recovery_cleanup_keeps_retry_input_while_backup_is_pending() -> TestResult {
    let backup = tempdir()?;
    let offline = backup.path().with_extension("recovery-offline");
    let mut fixture =
        Fixture::build_with_backup(AudioFormat::Flac, true, true, true, Some(backup.path()))?;
    let run = normalized(&mut fixture)?;
    std::fs::rename(backup.path(), &offline)?;
    let result = publish_new_album(
        &mut fixture.store,
        &fixture.claim,
        fixture.staging.as_ref().ok_or("staging")?,
        run,
    );
    std::fs::rename(&offline, backup.path())?;
    assert_eq!(result?, "backup_pending");
    assert!(
        fixture
            .scratch
            .path()
            .join(format!("normalization-{run}"))
            .is_dir()
    );
    assert!(fixture.store.recovery_operation(run * 8 + 1)?.is_none());
    let raw = Connection::open(&fixture.database)?;
    let count: i64 = raw.query_row("SELECT count(*) FROM scratch_cleanup_admissions", [], |r| {
        r.get(0)
    })?;
    assert_eq!(count, 0);
    let input: i64 = raw.query_row(
        "SELECT id FROM transfer_files ORDER BY id LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    assert!(
        fixture
            .scratch
            .path()
            .join(format!("transfer-{input}.input"))
            .is_file()
    );
    fixture.staging.take();
    assert!(continue_recovery(&mut fixture.store, run * 8 + 1, 0, true).is_err());
    Ok(())
}

#[test]
fn recovery_rejects_replaced_roots_without_adopting_their_files() -> TestResult {
    let (mut fixture, _) = restore_fixture()?;
    let entry = retained(&fixture)?;
    let decision = deletion(&mut fixture, entry.id)?;
    let moved = fixture
        ._library
        .path()
        .with_extension("recovery-root-original");
    std::fs::rename(fixture._library.path(), &moved)?;
    std::fs::create_dir(fixture._library.path())?;
    let marker = fixture._library.path().join("unrelated.txt");
    std::fs::write(&marker, b"unrelated replacement root")?;
    let result = delete_recovery(&mut fixture.store, entry.id, decision);
    assert_eq!(std::fs::read(&marker)?, b"unrelated replacement root");
    std::fs::remove_file(marker)?;
    std::fs::remove_dir(fixture._library.path())?;
    std::fs::rename(&moved, fixture._library.path())?;
    assert!(result.is_err());
    assert!(fixture.store.recovery_operation(entry.id)?.is_none());
    Ok(())
}
