//! Offline per-client reuse and stream-drop soak harness for Phase 4.

use std::fs;
use std::time::Instant;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{GenerateRequest, LlmClient, Message, ModelRef};

const ENDPOINT: &str = "http://127.0.0.1:41994/v1/chat/completions";
const RESOURCE_SAMPLE_INTERVAL: usize = 256;
const RELEASE_MAX_RSS_GROWTH_KIB: i64 = 64 * 1024;
const RELEASE_MAX_THREAD_GROWTH: i64 = 4;
const RELEASE_MAX_FD_GROWTH: i64 = 8;
const RELEASE_MAX_SOCKET_GROWTH: i64 = 4;

#[derive(Clone, Copy, Debug)]
struct ProcessSample {
    rss_kib: u64,
    threads: u64,
    file_descriptors: u64,
    sockets: u64,
}

impl ProcessSample {
    fn delta(self, baseline: Self) -> ProcessDelta {
        ProcessDelta {
            rss_kib: signed_delta(self.rss_kib, baseline.rss_kib),
            threads: signed_delta(self.threads, baseline.threads),
            file_descriptors: signed_delta(self.file_descriptors, baseline.file_descriptors),
            sockets: signed_delta(self.sockets, baseline.sockets),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ProcessDelta {
    rss_kib: i64,
    threads: i64,
    file_descriptors: i64,
    sockets: i64,
}

struct SoakResult {
    iterations: usize,
    completed: usize,
    dropped: usize,
    body_polls: usize,
    retained_captures: usize,
    elapsed_millis: u128,
}

fn signed_delta(current: u64, baseline: u64) -> i64 {
    i64::try_from(current).unwrap_or(i64::MAX) - i64::try_from(baseline).unwrap_or(i64::MAX)
}

#[cfg(target_os = "linux")]
fn process_sample() -> Option<ProcessSample> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let rss_kib = proc_status_value(&status, "VmRSS:")?;
    let threads = proc_status_value(&status, "Threads:")?;
    let mut file_descriptors = 0_u64;
    let mut sockets = 0_u64;
    for entry in fs::read_dir("/proc/self/fd").ok()? {
        let entry = entry.ok()?;
        file_descriptors += 1;
        if fs::read_link(entry.path())
            .ok()
            .is_some_and(|target| target.to_string_lossy().starts_with("socket:["))
        {
            sockets += 1;
        }
    }
    Some(ProcessSample {
        rss_kib,
        threads,
        file_descriptors,
        sockets,
    })
}

#[cfg(target_os = "linux")]
fn proc_status_value(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find(|line| line.starts_with(key))?
        .split_ascii_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn process_sample() -> Option<ProcessSample> {
    let _ = &fs::metadata(".");
    None
}

fn validate_release_delta(profile: &str, delta: ProcessDelta) {
    if profile != "release" {
        return;
    }
    assert!(
        delta.rss_kib <= RELEASE_MAX_RSS_GROWTH_KIB,
        "release soak RSS grew by {} KiB, limit is {RELEASE_MAX_RSS_GROWTH_KIB} KiB",
        delta.rss_kib
    );
    assert!(
        delta.threads <= RELEASE_MAX_THREAD_GROWTH,
        "release soak thread count grew by {}, limit is {RELEASE_MAX_THREAD_GROWTH}",
        delta.threads
    );
    assert!(
        delta.file_descriptors <= RELEASE_MAX_FD_GROWTH,
        "release soak file descriptor count grew by {}, limit is {RELEASE_MAX_FD_GROWTH}",
        delta.file_descriptors
    );
    assert!(
        delta.sockets <= RELEASE_MAX_SOCKET_GROWTH,
        "release soak socket count grew by {}, limit is {RELEASE_MAX_SOCKET_GROWTH}",
        delta.sockets
    );
}

fn print_result(profile: &str, result: &SoakResult, process_samples: &[ProcessSample]) {
    if let (Some(baseline), Some(final_sample)) = (
        process_samples.first().copied(),
        process_samples.last().copied(),
    ) {
        let delta = final_sample.delta(baseline);
        validate_release_delta(profile, delta);
        let max_rss_kib = process_samples
            .iter()
            .map(|sample| sample.rss_kib)
            .max()
            .unwrap_or(final_sample.rss_kib);
        println!(
            "{{\"profile\":\"{profile}\",\"iterations\":{},\"completed\":{},\"dropped\":{},\"body_polls\":{},\"retained_captures\":{},\"elapsed_millis\":{},\"resource_metrics_available\":true,\"resource_samples\":{},\"rss_start_kib\":{},\"rss_end_kib\":{},\"rss_max_kib\":{max_rss_kib},\"rss_growth_kib\":{},\"threads_start\":{},\"threads_end\":{},\"threads_growth\":{},\"fds_start\":{},\"fds_end\":{},\"fds_growth\":{},\"sockets_start\":{},\"sockets_end\":{},\"sockets_growth\":{}}}",
            result.iterations,
            result.completed,
            result.dropped,
            result.body_polls,
            result.retained_captures,
            result.elapsed_millis,
            process_samples.len(),
            baseline.rss_kib,
            final_sample.rss_kib,
            delta.rss_kib,
            baseline.threads,
            final_sample.threads,
            delta.threads,
            baseline.file_descriptors,
            final_sample.file_descriptors,
            delta.file_descriptors,
            baseline.sockets,
            final_sample.sockets,
            delta.sockets
        );
    } else {
        println!(
            "{{\"profile\":\"{profile}\",\"iterations\":{},\"completed\":{},\"dropped\":{},\"body_polls\":{},\"retained_captures\":{},\"elapsed_millis\":{},\"resource_metrics_available\":false}}",
            result.iterations,
            result.completed,
            result.dropped,
            result.body_polls,
            result.retained_captures,
            result.elapsed_millis
        );
    }
}

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("fixed soak fixture")],
    )
}

fn exchange(iteration: usize) -> MockExchange {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    let body = Bytes::from(format!(
        "data: {{\"id\":\"soak-{iteration}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"ok\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"id\":\"soak-{iteration}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
    ));
    MockExchange::response(MockResponse::new(
        StatusCode::OK,
        headers,
        vec![MockBodyItem::chunk(body)],
    ))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let profile = args.get(1).map_or("quick", String::as_str);
    let default_iterations = match profile {
        "quick" => 1_000,
        "release" => 100_000,
        "connection-churn" => 25_000,
        _ => panic!("profile must be quick, release, or connection-churn"),
    };
    let iterations = args
        .get(2)
        .map_or(default_iterations, |value| value.parse().unwrap());
    let mock = MockTransport::default();
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "soak-fixture-key")
        .unwrap()
        .build()
        .unwrap();
    let client = LlmClient::new(runtime, mock.clone());
    let started = Instant::now();
    let mut dropped = 0_usize;
    let mut completed = 0_usize;
    let mut process_samples = Vec::new();

    if let Some(sample) = process_sample() {
        process_samples.push(sample);
    }

    for iteration in 0..iterations {
        mock.push(exchange(iteration));
        if iteration % 10 == 0 {
            let stream = client.stream(request()).await.unwrap();
            drop(stream);
            dropped += 1;
        } else {
            let response = client.complete(request()).await.unwrap();
            assert_eq!(response.text(), "ok");
            completed += 1;
        }
        if iteration % RESOURCE_SAMPLE_INTERVAL == RESOURCE_SAMPLE_INTERVAL - 1 {
            mock.drain_captured_requests();
            tokio::task::yield_now().await;
            if let Some(sample) = process_sample() {
                process_samples.push(sample);
            }
        }
    }

    let retained_captures = mock.drain_captured_requests().len();
    assert_eq!(mock.remaining_expectations(), 0);
    assert_eq!(mock.early_body_drop_count(), dropped);
    assert!(retained_captures <= 255);
    let final_sample = process_sample();
    if let Some(sample) = final_sample {
        process_samples.push(sample);
    }

    print_result(
        profile,
        &SoakResult {
            iterations,
            completed,
            dropped,
            body_polls: mock.body_poll_count(),
            retained_captures,
            elapsed_millis: started.elapsed().as_millis(),
        },
        &process_samples,
    );
}
