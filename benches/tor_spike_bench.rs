use std::time::Duration;

use butterfly_bot::tor_spike::tor_http_get;
use criterion::{criterion_group, criterion_main, Criterion};

fn tor_host() -> String {
    std::env::var("TOR_SPIKE_HOST").unwrap_or_else(|_| "example.com".to_string())
}

fn tor_port() -> u16 {
    std::env::var("TOR_SPIKE_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(80)
}

fn tor_onion_host() -> Option<String> {
    std::env::var("TOR_SPIKE_ONION_HOST").ok()
}

fn tor_onion_port() -> u16 {
    std::env::var("TOR_SPIKE_ONION_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(80)
}

fn bench_tor_http_get(c: &mut Criterion) {
    let host = tor_host();
    let port = tor_port();

    let mut group = c.benchmark_group("tor_spike");
    group.measurement_time(Duration::from_secs(40));
    group.sample_size(10);

    group.bench_function("tor_http_get", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let _ = tor_http_get(&host, port).await.unwrap();
            });
        })
    });

    group.finish();
}

fn bench_tor_onion_http_get(c: &mut Criterion) {
    let Some(host) = tor_onion_host() else {
        return;
    };
    let port = tor_onion_port();

    let preflight = tokio::runtime::Runtime::new().unwrap();
    if let Err(err) = preflight.block_on(async { tor_http_get(&host, port).await }) {
        eprintln!("tor_onion_spike preflight failed: {err:?}");
        return;
    }

    let mut group = c.benchmark_group("tor_onion_spike");
    group.measurement_time(Duration::from_secs(60));
    group.sample_size(10);

    group.bench_function("tor_onion_http_get", |b| {
        b.iter(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let _ = tor_http_get(&host, port).await.unwrap();
            });
        })
    });

    group.finish();
}

criterion_group!(benches, bench_tor_http_get, bench_tor_onion_http_get);
criterion_main!(benches);
