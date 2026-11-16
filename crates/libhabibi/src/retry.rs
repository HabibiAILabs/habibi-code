//! Retry only before handing a response stream to an adapter. Never replay streamed output.
use crate::{Event, session::emit};
use anyhow::{Context, Result};
use reqwest::{RequestBuilder, Response, StatusCode};
use std::time::Duration;
use tokio::sync::mpsc;

const BACKOFF: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
];

fn transient(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

fn delay(response: Option<&Response>, backoff: Duration) -> Duration {
    // Honor delta-seconds Retry-After, bounded so a server cannot stall us indefinitely.
    // HTTP-date values fall back to our exponential schedule.
    let requested = response
        .and_then(|r| r.headers().get(reqwest::header::RETRY_AFTER))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_default();
    backoff.max(requested).min(Duration::from_secs(60))
}

pub(crate) async fn send(
    request: RequestBuilder,
    events: Option<&mpsc::Sender<Event>>,
) -> Result<Response> {
    send_with_backoff(request, events, &BACKOFF).await
}

async fn send_with_backoff(
    request: RequestBuilder,
    events: Option<&mpsc::Sender<Event>>,
    backoff: &[Duration],
) -> Result<Response> {
    for attempt in 0..=backoff.len() {
        let result = request
            .try_clone()
            .context("Provider request body cannot be retried")?
            .send()
            .await;
        let retryable = match &result {
            Ok(response) => transient(response.status()),
            Err(error) => error.is_connect() || error.is_timeout(),
        };
        if !retryable || attempt == backoff.len() {
            return result.context("Provider request failed");
        }
        let wait = delay(result.as_ref().ok(), backoff[attempt]);
        // Release failed responses/connections before sleeping, without reading error bodies.
        drop(result);
        if let Some(events) = events {
            emit(
                events,
                Event::ProviderRetry {
                    retry: attempt + 1,
                    max_retries: backoff.len(),
                    delay_secs: wait.as_secs(),
                },
            )
            .await?;
        }
        // Cancellation drops this future via the session's existing select! boundary.
        tokio::time::sleep(wait).await;
    }
    unreachable!("retry loop always returns on its final attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn server(statuses: Vec<u16>) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let task = tokio::spawn(async move {
            for status in statuses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 1024];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                seen.fetch_add(1, Ordering::SeqCst);
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            }
        });
        (url, count, task)
    }

    #[test]
    fn bounded_exponential_policy_and_statuses() {
        assert_eq!(BACKOFF.map(|d| d.as_secs()), [2, 4, 8, 16, 32]);
        for code in [408, 429, 500, 502, 503, 504, 529] {
            assert!(transient(StatusCode::from_u16(code).unwrap()));
        }
        for code in [200, 400, 401, 403, 404, 422, 501] {
            assert!(!transient(StatusCode::from_u16(code).unwrap()));
        }
        assert_eq!(delay(None, BACKOFF[0]), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn transient_failures_recover_with_notices() {
        let (url, count, task) = server(vec![503, 429, 200]).await;
        let (tx, mut rx) = mpsc::channel(8);
        let response = send_with_backoff(
            reqwest::Client::new()
                .post(url)
                .json(&serde_json::json!({"model":"test"})),
            Some(&tx),
            &[Duration::ZERO; 5],
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(count.load(Ordering::SeqCst), 3);
        for expected in 1..=2 {
            assert!(
                matches!(rx.try_recv().unwrap(), Event::ProviderRetry { retry, max_retries: 5, .. } if retry == expected)
            );
        }
        assert!(rx.try_recv().is_err());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn exhaustion_returns_final_status_and_permanent_errors_do_not_retry() {
        for (statuses, expected, requests) in [
            (vec![503; 6], 503, 6),
            (vec![401], 401, 1),
            (vec![400], 400, 1),
        ] {
            let (url, count, task) = server(statuses).await;
            let response =
                send_with_backoff(reqwest::Client::new().get(url), None, &[Duration::ZERO; 5])
                    .await
                    .unwrap();
            assert_eq!(response.status(), expected);
            assert_eq!(count.load(Ordering::SeqCst), requests);
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn accepted_but_broken_stream_is_not_replayed() {
        let (url, count, task) = server(vec![200]).await;
        let (tx, mut rx) = mpsc::channel(8);
        assert!(
            crate::ollama::Client::new(&url)
                .unwrap()
                .complete("test", "system", &[], &tx, false, &[])
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(rx.try_recv().is_err());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn connection_failures_exhaust_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let (tx, mut rx) = mpsc::channel(8);
        let error = send_with_backoff(
            reqwest::Client::new().get(url),
            Some(&tx),
            &[Duration::ZERO; 5],
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<reqwest::Error>().unwrap().is_connect());
        for expected in 1..=5 {
            assert!(
                matches!(rx.try_recv().unwrap(), Event::ProviderRetry { retry, .. } if retry == expected)
            );
        }
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn retry_after_is_respected_but_bounded() {
        for (header, expected) in [
            ("12", 12),
            ("999999", 60),
            ("0", 2),
            ("invalid", 2),
            ("Wed, 21 Oct 2015 07:28:00 GMT", 2),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                socket.write_all(format!("HTTP/1.1 429 Test\r\nContent-Length: 0\r\nRetry-After: {header}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let response = reqwest::get(url).await.unwrap();
            assert_eq!(
                delay(Some(&response), BACKOFF[0]),
                Duration::from_secs(expected)
            );
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn dropping_during_backoff_stops_requests() {
        let (url, count, server) = server(vec![503, 200]).await;
        let (tx, mut rx) = mpsc::channel(8);
        let task =
            tokio::spawn(async move { send(reqwest::Client::new().get(url), Some(&tx)).await });
        assert!(matches!(
            rx.recv().await.unwrap(),
            Event::ProviderRetry {
                retry: 1,
                delay_secs: 2,
                ..
            }
        ));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
