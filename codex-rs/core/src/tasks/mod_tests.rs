use super::SessionTask;
use super::SessionTaskContext;
use super::emit_turn_network_proxy_metric;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use codex_otel::MetricsClient;
use codex_otel::MetricsConfig;
use codex_otel::SessionTelemetry;
use codex_otel::TURN_NETWORK_PROXY_METRIC;
use codex_protocol::ThreadId;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::user_input::UserInput;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::metrics::data::AggregatedMetrics;
use opentelemetry_sdk::metrics::data::Metric;
use opentelemetry_sdk::metrics::data::MetricData;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
struct WaitForCancelTask;

impl SessionTask for WaitForCancelTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }

    fn span_name(&self) -> &'static str {
        "session_task.wait_for_cancel_test"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        input: Vec<UserInput>,
        cancellation_token: CancellationToken,
    ) -> Option<String> {
        let _ = (session, ctx, input);
        cancellation_token.cancelled().await;
        None
    }
}

fn test_session_telemetry() -> SessionTelemetry {
    let exporter = InMemoryMetricExporter::default();
    let metrics = MetricsClient::new(
        MetricsConfig::in_memory("test", "codex-core", env!("CARGO_PKG_VERSION"), exporter)
            .with_runtime_reader(),
    )
    .expect("in-memory metrics client");
    SessionTelemetry::new(
        ThreadId::new(),
        "gpt-5.1",
        "gpt-5.1",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "tty".to_string(),
        SessionSource::Cli,
    )
    .with_metrics_without_metadata_tags(metrics)
}

fn find_metric<'a>(resource_metrics: &'a ResourceMetrics, name: &str) -> &'a Metric {
    for scope_metrics in resource_metrics.scope_metrics() {
        for metric in scope_metrics.metrics() {
            if metric.name() == name {
                return metric;
            }
        }
    }
    panic!("metric {name} missing");
}

fn attributes_to_map<'a>(
    attributes: impl Iterator<Item = &'a KeyValue>,
) -> BTreeMap<String, String> {
    attributes
        .map(|kv| (kv.key.as_str().to_string(), kv.value.as_str().to_string()))
        .collect()
}

fn metric_point(resource_metrics: &ResourceMetrics) -> (BTreeMap<String, String>, u64) {
    let metric = find_metric(resource_metrics, TURN_NETWORK_PROXY_METRIC);
    match metric.data() {
        AggregatedMetrics::U64(data) => match data {
            MetricData::Sum(sum) => {
                let points: Vec<_> = sum.data_points().collect();
                assert_eq!(points.len(), 1);
                let point = points[0];
                (attributes_to_map(point.attributes()), point.value())
            }
            _ => panic!("unexpected counter aggregation"),
        },
        _ => panic!("unexpected counter data type"),
    }
}

#[tokio::test]
async fn externally_cancelled_task_clears_active_turn_and_emits_abort() {
    let (session, turn_context, rx_events) =
        crate::session::tests::make_session_and_context_with_rx().await;

    session
        .spawn_task(Arc::clone(&turn_context), Vec::new(), WaitForCancelTask)
        .await;
    let task_cancellation_token = {
        let active_turn = session.active_turn.lock().await;
        active_turn
            .as_ref()
            .expect("active turn")
            .tasks
            .get(&turn_context.sub_id)
            .expect("running task")
            .cancellation_token
            .clone()
    };

    task_cancellation_token.cancel();

    let aborted = timeout(Duration::from_secs(2), async {
        loop {
            let event = rx_events.recv().await.expect("session event");
            if let EventMsg::TurnAborted(aborted) = event.msg {
                return aborted;
            }
        }
    })
    .await
    .expect("timed out waiting for TurnAborted");

    assert_eq!(aborted.reason, TurnAbortReason::Interrupted);
    assert!(session.active_turn.lock().await.is_none());
}

#[test]
fn emit_turn_network_proxy_metric_records_active_turn() {
    let session_telemetry = test_session_telemetry();

    emit_turn_network_proxy_metric(
        &session_telemetry,
        /*network_proxy_active*/ true,
        ("tmp_mem_enabled", "true"),
    );

    let snapshot = session_telemetry
        .snapshot_metrics()
        .expect("runtime metrics snapshot");
    let (attrs, value) = metric_point(&snapshot);

    assert_eq!(value, 1);
    assert_eq!(
        attrs,
        BTreeMap::from([
            ("active".to_string(), "true".to_string()),
            ("tmp_mem_enabled".to_string(), "true".to_string()),
        ])
    );
}

#[test]
fn emit_turn_network_proxy_metric_records_inactive_turn() {
    let session_telemetry = test_session_telemetry();

    emit_turn_network_proxy_metric(
        &session_telemetry,
        /*network_proxy_active*/ false,
        ("tmp_mem_enabled", "false"),
    );

    let snapshot = session_telemetry
        .snapshot_metrics()
        .expect("runtime metrics snapshot");
    let (attrs, value) = metric_point(&snapshot);

    assert_eq!(value, 1);
    assert_eq!(
        attrs,
        BTreeMap::from([
            ("active".to_string(), "false".to_string()),
            ("tmp_mem_enabled".to_string(), "false".to_string()),
        ])
    );
}
