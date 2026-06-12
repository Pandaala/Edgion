//! Unit tests for `workqueue.rs` (see `super`).
use super::*;

#[tokio::test]
async fn test_workqueue_lifecycle_scenarios() {
    // scenario: baseline enqueue + dequeue
    {
        let queue = Workqueue::with_defaults("test");

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);

        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/name");
        assert!(item.trigger_chain.sources.is_empty());
        assert_eq!(queue.len(), 0);

        queue.done(&item.key);
    }

    // scenario: dedup — second enqueue of same key returns false
    {
        let queue = Workqueue::with_defaults("test");

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert!(!queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);
    }

    // scenario: re-enqueue while item is in-flight (after dequeue, before done)
    {
        // lifecycle phase: item dequeued but not yet done — key removed from pending,
        // so a new enqueue of the same key must succeed
        let queue = Workqueue::with_defaults("test");

        queue.enqueue("ns/name".to_string()).await;
        let _item = queue.dequeue().await.unwrap();

        assert!(queue.enqueue("ns/name".to_string()).await);
        assert_eq!(queue.len(), 1);
    }

    // scenario: done releases the slot for re-enqueue
    {
        let queue = Workqueue::with_defaults("test");

        queue.enqueue("ns/name".to_string()).await;
        let item = queue.dequeue().await.unwrap();
        queue.done(&item.key);

        assert!(queue.enqueue("ns/name".to_string()).await);
    }

    // scenario: dirty requeue — re-enqueue lands while in-flight, second dequeue returns same key
    {
        // lifecycle phase: item is in-flight (dequeued, not yet done) when the same key
        // is re-enqueued; done() must promote the dirty entry so the next dequeue sees it
        let queue = Workqueue::with_defaults("test");

        queue.enqueue("ns/resource".to_string()).await;

        let item = queue.dequeue().await.unwrap();
        assert_eq!(item.key, "ns/resource");

        assert!(queue.enqueue("ns/resource".to_string()).await);

        queue.done(&item.key);

        assert_eq!(queue.len(), 1);

        let item2 = queue.dequeue().await.unwrap();
        assert_eq!(item2.key, "ns/resource");
        queue.done(&item2.key);

        assert_eq!(queue.len(), 0);
    }
}

#[tokio::test]
async fn test_metrics() {
    let queue = Workqueue::with_defaults("test");

    assert_eq!(queue.metrics().get_adds_total(), 0);
    assert_eq!(queue.metrics().get_depth(), 0);

    queue.enqueue("ns/name1".to_string()).await;
    queue.enqueue("ns/name2".to_string()).await;

    assert_eq!(queue.metrics().get_adds_total(), 2);
    assert_eq!(queue.metrics().get_depth(), 2);

    let item = queue.dequeue().await.unwrap();
    assert_eq!(queue.metrics().get_depth(), 1);

    queue.done(&item.key);
    assert_eq!(queue.metrics().get_depth(), 1);
}

#[tokio::test]
async fn test_concurrent_enqueue_dedup() {
    let queue = Arc::new(Workqueue::with_defaults("test"));
    let num_tasks = 20;
    let mut handles = vec![];

    for _ in 0..num_tasks {
        let q = queue.clone();
        let handle = tokio::spawn(async move { q.enqueue("ns/hotkey".to_string()).await });
        handles.push(handle);
    }

    let mut success_count = 0usize;
    for handle in handles {
        if handle.await.unwrap() {
            success_count += 1;
        }
    }
    assert_eq!(success_count, 1, "Exactly one enqueue should win the race");

    assert_eq!(
        queue.len(),
        1,
        "Queue should contain exactly one item after concurrent enqueues"
    );
    assert_eq!(queue.metrics().get_depth(), 1, "Metrics depth should be 1");
    assert_eq!(queue.metrics().get_adds_total(), 1, "Only 1 add should be counted");

    let item = queue.dequeue().await.unwrap();
    assert_eq!(item.key, "ns/hotkey");
    assert_eq!(queue.len(), 0);
    assert_eq!(queue.metrics().get_depth(), 0);
}

#[tokio::test]
async fn test_concurrent_enqueue_after_and_enqueue() {
    let queue = Arc::new(Workqueue::with_defaults("test"));
    let queue_clone1 = queue.clone();
    let queue_clone2 = queue.clone();

    // Concurrently call enqueue_after (with 100ms delay) and enqueue (immediate)
    // on the same key. This tests that:
    // 1. Both calls succeed in queueing the key (once in scheduled, once in pending)
    // 2. When we dequeue the immediate item, the pending is clear
    // 3. When the delay fires, it will add the key to pending again (since pending is empty)
    let enqueue_after_handle = tokio::spawn(async move {
        queue_clone1
            .enqueue_after("ns/key".to_string(), Duration::from_millis(100), TriggerChain::new())
            .await
    });

    let enqueue_handle = tokio::spawn(async move { queue_clone2.enqueue("ns/key".to_string()).await });

    let scheduled = enqueue_after_handle.await.unwrap();
    let enqueued = enqueue_handle.await.unwrap();

    // Both succeed independently — enqueue writes to pending, enqueue_after writes to scheduled.
    // Since they target independent DashSets, both operations always succeed.
    assert!(enqueued, "immediate enqueue should succeed");
    assert!(scheduled, "enqueue_after should succeed");

    // Dequeue the immediate item from the successful enqueue path
    let item = queue.dequeue().await.expect("Should have an item");
    assert_eq!(item.key, "ns/key");

    // Now pending is empty. Wait for the delay to fire.
    sleep(Duration::from_millis(150)).await;

    // The delay loop will attempt to promote the scheduled item.
    // Since pending is now empty (we dequeued it above), the pending.insert succeeds
    // and the delayed item is promoted to the ready queue.
    let item2 = tokio::time::timeout(Duration::from_millis(500), queue.dequeue())
        .await
        .expect("Timeout waiting for delayed item")
        .expect("Queue should not be closed");
    assert_eq!(item2.key, "ns/key", "Delayed item should be the same key");

    // Now all should be processed
    assert_eq!(queue.len(), 0, "Pending queue should be empty after draining");
}

#[tokio::test]
async fn test_delay_fire_collision_preserves_chain() {
    // Reproduces c3a-008: when enqueue_after's delay fires and the key is already
    // pending from a concurrent immediate enqueue, the trigger chain must be
    // merged into the pending entry, not silently dropped. Otherwise the cycle
    // detector loses cascade history and `max_trigger_cycles` cannot fire.
    let queue = Workqueue::with_defaults("test");
    let chain = TriggerChain::new().extend("HTTPRoute", "default/route-x");
    assert_eq!(chain.depth(), 1);

    // Schedule with delay (50ms) carrying a chain of depth 1.
    queue
        .enqueue_after("ns/key".to_string(), Duration::from_millis(50), chain.clone())
        .await;

    // Immediate enqueue lands in pending with empty chain. The delayed item is
    // still parked in the heap; when its 50ms timer fires it will hit the
    // pending-collision branch.
    queue.enqueue("ns/key".to_string()).await;

    // Wait for the delay to fire while the immediate item is still pending
    // (we deliberately do NOT dequeue between the two enqueues).
    sleep(Duration::from_millis(120)).await;

    // Dequeue: the surviving WorkItem must carry the merged chain (depth 1),
    // not the empty chain that would result if the collision dropped the chain.
    let item = queue.dequeue().await.expect("Should have an item");
    assert_eq!(item.key, "ns/key");
    assert_eq!(
        item.trigger_chain.depth(),
        1,
        "Chain must be preserved when delay-fire collides with pending"
    );
    assert_eq!(item.trigger_chain.sources[0].kind, "HTTPRoute");
    assert_eq!(item.trigger_chain.sources[0].key, "default/route-x");

    assert_eq!(queue.len(), 0, "No more items after dequeue");
}

/// High-water depth tracking: latch must flip exactly on the entry edge
/// (depth >= 80% of capacity), stay latched while above, recover below
/// the 70% recovery threshold, and re-trigger on a fresh entry.
///
/// Replaces the broken `depth % 500 == 0` rate-limiter (`c3a-014`) which
/// would miss most high-load events because the warn only fired when
/// depth happened to land on a 500-multiple.
#[tokio::test]
async fn test_high_water_edge_trigger_and_recovery() {
    let queue = Workqueue::new(
        "test-hw",
        WorkqueueConfig {
            capacity: 10,
            ..Default::default()
        },
    );

    // depth 1..=7: below 80% threshold, latch stays false
    for i in 0..7 {
        queue.enqueue(format!("k{i}")).await;
        assert!(
            !queue.metrics().is_high_water(),
            "high_water must stay false at depth {}",
            i + 1
        );
    }
    assert_eq!(queue.metrics().get_depth(), 7);

    // depth 8 (== 80% of 10): entry edge — latch flips to true
    queue.enqueue("k_enter".to_string()).await;
    assert_eq!(queue.metrics().get_depth(), 8);
    assert!(
        queue.metrics().is_high_water(),
        "high_water must enter at depth 8 (80% of capacity)"
    );

    // depth 9: latch already held; no spurious double-trigger.
    // (We can't directly observe "warn fired N times" without a tracing
    // subscriber, but the latch state proves the compare_exchange gate
    // would have rejected a second warn.)
    queue.enqueue("k_above".to_string()).await;
    assert_eq!(queue.metrics().get_depth(), 9);
    assert!(queue.metrics().is_high_water());

    // dec to 8: still in high-water region (8 > 7 recovery threshold)
    let _ = queue.dequeue().await.expect("dequeue");
    assert_eq!(queue.metrics().get_depth(), 8);
    assert!(
        queue.metrics().is_high_water(),
        "must remain latched at depth 8 (above 70% recovery threshold)"
    );

    // dec to 7 (== 70% of 10): recovery edge — latch flips to false
    let _ = queue.dequeue().await.expect("dequeue");
    assert_eq!(queue.metrics().get_depth(), 7);
    assert!(
        !queue.metrics().is_high_water(),
        "high_water must recover at depth 7 (70% of capacity)"
    );

    // Further decs: no state change, no spurious recovery info
    let _ = queue.dequeue().await.expect("dequeue");
    assert_eq!(queue.metrics().get_depth(), 6);
    assert!(!queue.metrics().is_high_water());

    // Re-entry: depth climbing back to 8 must trigger the latch again.
    // This proves the recovery side correctly cleared the latch so it
    // can fire on the next crossing (vs a one-shot that never re-arms).
    queue.enqueue("k_re_a".to_string()).await;
    queue.enqueue("k_re_b".to_string()).await;
    assert_eq!(queue.metrics().get_depth(), 8);
    assert!(
        queue.metrics().is_high_water(),
        "high_water must re-enter on a fresh crossing after recovery"
    );
}

/// Capacity 0 (degenerate config) must be a no-op for the watermark check
/// and never panic on the `cap * PERCENT` multiplication. `WorkqueueConfig`
/// forbids `capacity=0` in practice (`mpsc::channel(0)` panics), but the
/// helper itself must defend against it for any future direct caller.
#[tokio::test]
async fn test_high_water_zero_capacity_is_noop() {
    let metrics = WorkqueueMetrics::new();
    metrics.check_high_water_on_inc("test", 100, 0);
    assert!(!metrics.is_high_water());
    metrics.check_high_water_on_dec("test", 0, 0);
    assert!(!metrics.is_high_water());
}
