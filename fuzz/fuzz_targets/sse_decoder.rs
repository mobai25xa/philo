#![no_main]

use bytes::Bytes;
use futures_util::{StreamExt as _, stream};
use libfuzzer_sys::fuzz_target;
use philo::transport::{ByteStream, SseConfig, SseDecoder};

fuzz_target!(|data: &[u8]| {
    let chunk_size = 1 + data.first().copied().unwrap_or_default() as usize % 31;
    let chunks = data
        .chunks(chunk_size)
        .map(Bytes::copy_from_slice)
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let upstream: ByteStream = Box::pin(stream::iter(chunks.into_iter().map(Ok)));
        let config = SseConfig::new(64 * 1024, 16 * 1024).expect("static limits");
        let mut decoder = SseDecoder::with_config(upstream, config);
        while let Some(result) = decoder.next().await {
            let _ = result;
        }
    });
});
