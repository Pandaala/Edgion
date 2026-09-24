use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use mockall::Sequence;

fn config(status_code: u16, body: &str, terminate: bool) -> MockConfig {
    MockConfig {
        status_code,
        body: Some(body.to_string()),
        headers: Some(HashMap::from([("X-Mock".to_string(), "yes".to_string())])),
        content_type: "text/plain".to_string(),
        delay: None,
        terminate,
    }
}

#[test]
fn name_is_static_across_instances() {
    let first = Mock::new(&MockConfig::default());
    let second = Mock::new(&MockConfig::default());

    assert_eq!(first.name(), PLUGIN_NAME);
    assert_eq!(second.name(), PLUGIN_NAME);
    assert!(std::ptr::eq(first.name(), second.name()));
}

#[test]
fn prepared_generations_do_not_retain_or_share_caller_config() {
    let mut first_config = config(201, "first", true);
    first_config.delay = Some(25);
    let first = Mock::new(&first_config);

    first_config.status_code = 503;
    first_config.body = Some("mutated".to_string());
    first_config
        .headers
        .as_mut()
        .unwrap()
        .insert("X-Later".to_string(), "changed".to_string());
    first_config.content_type = "application/json".to_string();
    first_config.delay = Some(99);
    first_config.terminate = false;

    let second = Mock::new(&config(202, "second", false));

    assert_eq!(first.status_code, 201);
    assert_eq!(first.body.as_deref(), Some(&b"first"[..]));
    assert_eq!(first.headers.as_ref().unwrap().len(), 1);
    assert_eq!(first.content_type, "text/plain");
    assert_eq!(first.delay, Some(Duration::from_millis(25)));
    assert!(first.terminate);
    assert_eq!(first.status_log, "mock served 201");

    assert_eq!(second.status_code, 202);
    assert_eq!(second.body.as_deref(), Some(&b"second"[..]));
    assert!(!second.terminate);
    assert_eq!(second.status_log, "mock served 202");
}

#[tokio::test]
async fn terminating_response_preserves_header_order_result_and_log() {
    let mock = Mock::new(&config(201, "created", true));
    let mut session = MockPluginSession::new();
    let mut sequence = Sequence::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "X-Mock" && value == "yes")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_set_response_header()
        .withf(|name, value| name == "Content-Type" && value == "text/plain")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));

    let mut log = PluginLog::new(PLUGIN_NAME);
    let result = mock.run_request(&mut session, &mut log).await;

    match result {
        PluginRunningResult::ErrResponse { status, body } => {
            assert_eq!(status, 201);
            assert_eq!(body.as_deref(), Some(&b"created"[..]));
        }
        other => panic!("unexpected result: {other:?}"),
    }
    assert_eq!(log.render_for_test(), "mock served 201");
}

/// The point of the `Bytes` body: a generation-owned configured body is
/// shared across requests, not copied. Two independent invocations must
/// hand out views over the *same* allocation the plugin prepared once.
///
/// The pointer comparison is only sound while `mock` (and therefore the
/// prepared allocation) is alive for the whole test — a live allocation
/// cannot be aliased by a fresh one, so allocator reuse cannot fake a pass.
/// Do not restructure this to drop `mock` before the assertions.
#[tokio::test]
async fn terminating_body_is_shared_across_requests_not_copied() {
    let mock = Mock::new(&config(200, "a stable configured body", true));
    let prepared = mock.body.as_ref().expect("configured body").as_ptr();

    let mut bodies = Vec::new();
    for _ in 0..2 {
        let mut session = MockPluginSession::new();
        session.expect_set_response_header().returning(|_, _| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);
        match mock.run_request(&mut session, &mut log).await {
            PluginRunningResult::ErrResponse { body: Some(b), .. } => bodies.push(b),
            other => panic!("unexpected result: {other:?}"),
        }
    }

    assert_eq!(bodies[0].as_ref(), b"a stable configured body");
    assert_eq!(bodies[0], bodies[1]);
    // Same backing buffer as the prepared field, for every request.
    assert_eq!(bodies[0].as_ptr(), prepared);
    assert_eq!(bodies[1].as_ptr(), prepared);
}

#[tokio::test]
async fn non_terminating_response_sets_headers_then_continues() {
    let mock = Mock::new(&config(503, "ignored", false));
    let mut session = MockPluginSession::new();
    let mut sequence = Sequence::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "X-Mock" && value == "yes")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_set_response_header()
        .withf(|name, value| name == "Content-Type" && value == "text/plain")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));

    let mut log = PluginLog::new(PLUGIN_NAME);
    let result = mock.run_request(&mut session, &mut log).await;

    assert!(matches!(result, PluginRunningResult::GoodNext));
    assert_eq!(log.render_for_test(), "mock headers set");
}

#[tokio::test(start_paused = true)]
async fn delay_completes_before_response_headers_are_set() {
    let mut mock_config = config(200, "delayed", true);
    mock_config.delay = Some(100);
    let mock = Mock::new(&mock_config);
    let mut session = MockPluginSession::new();
    let start = tokio::time::Instant::now();
    let mut sequence = Sequence::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "X-Mock" && value == "yes")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |_, _| {
            assert_eq!(
                tokio::time::Instant::now().duration_since(start),
                Duration::from_millis(100)
            );
            Ok(())
        });
    session
        .expect_set_response_header()
        .withf(|name, value| name == "Content-Type" && value == "text/plain")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(move |_, _| {
            assert_eq!(
                tokio::time::Instant::now().duration_since(start),
                Duration::from_millis(100)
            );
            Ok(())
        });

    let mut log = PluginLog::new(PLUGIN_NAME);
    let result = {
        let run = mock.run_request(&mut session, &mut log);
        tokio::pin!(run);

        tokio::select! {
            result = &mut run => panic!("mock completed before its delay: {result:?}"),
            () = sleep(Duration::from_millis(99)) => {}
        }
        assert_eq!(
            tokio::time::Instant::now().duration_since(start),
            Duration::from_millis(99)
        );

        run.await
    };
    assert_eq!(
        tokio::time::Instant::now().duration_since(start),
        Duration::from_millis(100)
    );
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 200, .. }));
    assert_eq!(log.render_for_test(), "mock served 200");
}
