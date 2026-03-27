use openjarvis::{agent::AgentWorkerHandle, router::ChannelRouter, session::SessionManager};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, timeout},
};

async fn wait_for_test_shutdown(shutdown_rx: oneshot::Receiver<()>) {
    let _ = shutdown_rx.await;
}

#[tokio::test]
async fn router_run_stays_pending_while_service_is_healthy() {
    let (request_tx, _request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive to model a healthy service.
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(SessionManager::new())
        .build()
        .expect("router should build");

    let result = timeout(Duration::from_millis(50), router.run()).await;

    assert!(
        result.is_err(),
        "router.run should remain pending while shutdown is not requested"
    );

    drop(event_tx);
}

#[tokio::test]
async fn router_run_until_shutdown_exits_when_shutdown_signal_arrives() {
    let (request_tx, _request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: keeps the downstream event channel alive to model a healthy service.
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(SessionManager::new())
        .build()
        .expect("router should build");
    let (shutdown_tx, shutdown_rx) = oneshot::channel(); // test-only: drives router shutdown explicitly.
    let shutdown_task = tokio::spawn(async move {
        shutdown_tx
            .send(())
            .expect("test shutdown should be delivered");
    });

    timeout(
        Duration::from_millis(50),
        router.run_until_shutdown(wait_for_test_shutdown(shutdown_rx)),
    )
    .await
    .expect("router should exit after shutdown signal")
    .expect("router loop should return ok");
    shutdown_task.await.expect("shutdown task should complete");
    drop(event_tx);
}

#[tokio::test]
async fn router_returns_error_when_agent_event_channel_closes() {
    let (request_tx, _request_rx) = mpsc::channel(8);
    let (event_tx, event_rx) = mpsc::channel(8); // test-only: drop this sender to simulate a crashed downstream worker.
    let mut router = ChannelRouter::builder()
        .agent_handle(AgentWorkerHandle {
            request_tx,
            event_rx,
        })
        .session_manager(SessionManager::new())
        .build()
        .expect("router should build");

    drop(event_tx);

    let error = router
        .run()
        .await
        .expect_err("router should surface a downstream worker disconnect");

    assert!(
        error
            .to_string()
            .contains("agent worker event channel closed")
    );
}
