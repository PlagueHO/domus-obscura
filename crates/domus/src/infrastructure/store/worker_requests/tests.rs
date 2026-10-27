use super::*;
use crate::application::add_location;
use std::sync::{Arc, Barrier};

const BINDING: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn fixture() -> Result<(tempfile::TempDir, Store, Vec<i64>)> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(&temp.path().join("state.sqlite"))?;
    let mut locations = Vec::new();
    for (index, name) in ["first", "second", "third"].into_iter().enumerate() {
        let root = temp.path().join(name);
        std::fs::create_dir(&root)?;
        locations.push(add_location(
            &store,
            &root,
            if index == 0 { "main" } else { "backup" },
        )?);
    }
    Ok((temp, store, locations))
}

#[test]
fn worker_queue_is_bounded_idempotent_and_available_offline() -> Result<()> {
    let (_temp, mut store, locations) = fixture()?;
    let operation = WorkerOperation::Scan {
        location_id: locations[0],
    };
    let first = store.enqueue_worker(operation.clone(), 1, BINDING)?;
    assert_eq!(
        store.enqueue_worker(operation.clone(), 1, BINDING)?.id,
        first.id
    );
    assert!(matches!(
        store.enqueue_worker(operation, 1, &"b".repeat(64)),
        Err(Error::Conflict(_))
    ));
    for capacity in [0, 1025] {
        assert!(
            store
                .enqueue_worker(
                    WorkerOperation::Scan {
                        location_id: locations[1]
                    },
                    capacity,
                    BINDING
                )
                .is_err()
        );
    }
    assert!(matches!(
        store.enqueue_worker(
            WorkerOperation::Scan {
                location_id: locations[1]
            },
            1,
            BINDING
        ),
        Err(Error::Busy(_))
    ));
    assert!(!store.worker_status(1)?.online);
    assert_eq!(store.worker_status(1)?.unfinished, 1);
    assert!(
        store
            .connection
            .execute("UPDATE worker_requests SET body='{}'", [])
            .is_err()
    );
    Ok(())
}

#[test]
fn worker_waiting_request_does_not_block_eligible_work_or_retry_itself() -> Result<()> {
    let (_temp, mut store, locations) = fixture()?;
    let first = store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: locations[0],
        },
        3,
        BINDING,
    )?;
    let second = store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: locations[1],
        },
        3,
        BINDING,
    )?;
    let host = store.start_worker_host("fixture", BINDING)?;
    let claim = store
        .claim_worker_request(&host, "scan")?
        .ok_or(Error::Internal("missing first request"))?;
    assert_eq!(claim.request.id, first.id);
    store.finish_worker_request(&claim, "waiting", Some("saved question"))?;
    let next = store
        .claim_worker_request(&host, "scan")?
        .ok_or(Error::Internal("missing second request"))?;
    assert_eq!(next.request.id, second.id);
    store.finish_worker_request(&next, "complete", None)?;
    let unfinished = store.unfinished_worker_requests(Page::new(0, 10)?, None)?;
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].id, first.id);
    assert!(
        matches!(
            store.enqueue_worker(
                WorkerOperation::Scan {
                    location_id: locations[2],
                },
                1,
                BINDING
            ),
            Err(Error::Busy(_))
        ),
        "lowering capacity must retain waiting work and refuse new admission"
    );
    assert!(store.claim_worker_request(&host, "scan")?.is_none());
    store.retry_worker(first.id)?;
    assert_eq!(
        store
            .claim_worker_request(&host, "scan")?
            .ok_or(Error::Internal("missing explicit retry"))?
            .request
            .id,
        first.id
    );
    Ok(())
}

#[test]
fn worker_claims_fence_expired_requests_and_hosts() -> Result<()> {
    let (_temp, mut store, locations) = fixture()?;
    store.enqueue_worker(
        WorkerOperation::Scan {
            location_id: locations[0],
        },
        2,
        BINDING,
    )?;
    let host = store.start_worker_host("same-owner", BINDING)?;
    let old = store
        .claim_worker_request(&host, "scan")?
        .ok_or(Error::Internal("missing claim"))?;
    store
        .connection
        .execute("UPDATE worker_requests SET lease_until=unixepoch()-1", [])?;
    assert_eq!(store.worker_status(2)?.active, 0);
    assert!(store.renew_worker_request(&old).is_err());
    let current = store
        .claim_worker_request(&host, "scan")?
        .ok_or(Error::Internal("missing reclaimed claim"))?;
    assert!(store.finish_worker_request(&old, "complete", None).is_err());
    store.finish_worker_request(&current, "failed", Some("operator action required"))?;
    assert!(store.claim_worker_request(&host, "scan")?.is_none());
    let new_host = store.start_worker_host("same-owner", &"b".repeat(64))?;
    assert!(store.heartbeat_worker(&host, false).is_err());
    assert!(store.stop_worker_host(&host).is_err());
    assert!(store.claim_worker_request(&host, "scan").is_err());
    store.heartbeat_worker(&new_host, true)?;
    assert!(store.claim_worker_request(&new_host, "scan").is_err());
    store
        .connection
        .execute("UPDATE worker_host SET heartbeat=unixepoch()-16", [])?;
    assert!(!store.worker_status(2)?.stopping);
    store.dismiss_worker(current.request.id)?;
    assert_eq!(store.worker_status(2)?.unfinished, 0);
    Ok(())
}

#[test]
fn worker_capacity_is_atomic_across_database_connections() -> Result<()> {
    let (temp, store, locations) = fixture()?;
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let handles = locations[..2]
        .iter()
        .map(|location| {
            let database = temp.path().join("state.sqlite");
            let location = *location;
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = Store::open_current_existing(&database)?;
                barrier.wait();
                store.enqueue_worker(
                    WorkerOperation::Scan {
                        location_id: location,
                    },
                    1,
                    BINDING,
                )
            })
        })
        .collect::<Vec<_>>();
    let mut accepted = 0;
    for handle in handles {
        match handle
            .join()
            .map_err(|_| Error::Internal("admission thread panicked"))?
        {
            Ok(_) => accepted += 1,
            Err(Error::Busy(_)) => {}
            Err(error) => return Err(error),
        }
    }
    assert_eq!(accepted, 1);
    Ok(())
}
