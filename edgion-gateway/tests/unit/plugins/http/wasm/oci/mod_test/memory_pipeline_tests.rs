use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const ARTIFACT: &[u8] = b"verified-artifact";

fn local_cache() -> Arc<cache::OciMemoryCache> {
    let cache = cache::OciMemoryCache::new(cache::OciMemoryCacheConfig {
        max_bytes: 1024 * 1024,
        max_entries: 16,
        ttl: Duration::from_secs(60),
        purge_interval: Duration::from_secs(60),
    })
    .unwrap();
    cache.register_owner();
    cache
}

fn digest() -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(ARTIFACT)))
}

async fn lease() -> Arc<OciPipeline> {
    Arc::new(OciPipeline {
        _network: Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap(),
        _preparation: Arc::new(Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap()),
    })
}

#[tokio::test]
async fn concurrent_digest_misses_fetch_once_and_share_verified_allocation() {
    for kind in [CacheKind::Manifest, CacheKind::Blob] {
        let cache = local_cache();
        let digest = digest();
        let calls = AtomicUsize::new(0);
        let load = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(ARTIFACT.to_vec())
        };
        let (a, b) = (lease().await, lease().await);
        let (first, second) = tokio::join!(
            cached_verified_fetch(cache.access(), kind, &digest, &a, load),
            cached_verified_fetch(cache.access(), kind, &digest, &b, load),
        );
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(first.as_ref(), ARTIFACT);
        assert_eq!(first.as_ptr(), second.as_ptr());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn failed_owner_releases_claim_and_errors_or_mismatch_are_not_cached() {
    let cache = local_cache();
    let digest = digest();
    let lease = lease().await;
    let error = cached_verified_fetch(cache.access(), CacheKind::Blob, &digest, &lease, || async {
        Err("registry denied".to_string())
    })
    .await
    .unwrap_err();
    assert_eq!(error, "registry denied");
    let error = cached_verified_fetch(cache.access(), CacheKind::Blob, &digest, &lease, || async {
        Ok(b"corrupt".to_vec())
    })
    .await
    .unwrap_err();
    assert!(error.contains("digest mismatch"));
    assert!(cache
        .access()
        .load(CacheKind::Blob, digest.strip_prefix("sha256:").unwrap())
        .is_none());
    let valid = cached_verified_fetch(cache.access(), CacheKind::Blob, &digest, &lease, || async {
        Ok(ARTIFACT.to_vec())
    })
    .await
    .unwrap();
    assert_eq!(valid.as_ref(), ARTIFACT);
}

#[tokio::test]
async fn cancelled_fetch_owner_allows_waiter_to_retry() {
    let cache = local_cache();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let access = cache.access();
    let owner = tokio::spawn(async move {
        let lease = lease().await;
        cached_verified_fetch(access, CacheKind::Blob, &digest(), &lease, || async {
            started_tx.send(()).unwrap();
            std::future::pending().await
        })
        .await
    });
    started_rx.await.unwrap();
    let access = cache.access();
    let waiter = tokio::spawn(async move {
        let lease = lease().await;
        cached_verified_fetch(access, CacheKind::Blob, &digest(), &lease, || async {
            Ok(ARTIFACT.to_vec())
        })
        .await
    });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    let value = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(value.as_ref(), ARTIFACT);
}

#[tokio::test]
async fn abandoned_blocking_stage_keeps_pipeline_capacity_until_work_finishes() {
    // Production precompile hashing/extraction retains both admission owners.
    // Leave a real blocking task alive after its async owner is cancelled.
    let permits = Arc::new(Semaphore::new(1));
    let preparations = Arc::new(Semaphore::new(1));
    #[derive(Debug)]
    struct CheckedOutput {
        permits: Arc<Semaphore>,
        preparations: Arc<Semaphore>,
        dropped: Arc<AtomicUsize>,
    }
    impl Drop for CheckedOutput {
        fn drop(&mut self) {
            assert_eq!(self.permits.available_permits(), 0, "output must drop before its lease");
            assert_eq!(
                self.preparations.available_permits(),
                0,
                "precompile result retains global admission"
            );
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let output = CheckedOutput {
        permits: permits.clone(),
        preparations: preparations.clone(),
        dropped: dropped.clone(),
    };
    let lease = Arc::new(OciPipeline {
        _network: permits.clone().acquire_owned().await.unwrap(),
        _preparation: Arc::new(preparations.clone().acquire_owned().await.unwrap()),
    });
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        blocking_stage(&lease, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
            Ok(output)
        })
        .await
    });
    started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(permits.available_permits(), 0);
    assert_eq!(preparations.available_permits(), 0);
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    let permit = tokio::time::timeout(Duration::from_secs(1), permits.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert_eq!(permits.available_permits(), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    let permit = tokio::time::timeout(Duration::from_secs(1), preparations.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert_eq!(preparations.available_permits(), 1);
}

#[tokio::test]
async fn late_success_after_owner_clear_cannot_admit_into_new_generation() {
    let cache = local_cache();
    let access = cache.access();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let lease = lease().await;
        cached_verified_fetch(access, CacheKind::Blob, &digest(), &lease, || async {
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(ARTIFACT.to_vec())
        })
        .await
    });
    started_rx.await.unwrap();
    cache.unregister_owner();
    cache.register_owner();
    release_tx.send(()).unwrap();
    assert_eq!(task.await.unwrap().unwrap().as_ref(), ARTIFACT);
    assert!(cache
        .access()
        .load(CacheKind::Blob, digest().strip_prefix("sha256:").unwrap())
        .is_none());
}
