# Empirical Performance Benchmarks (`kadraj`)

See [BENCHMARKS.md](../BENCHMARKS.md) for the full 1,900-page static site benchmark methodology.

| Subsystem / Operation | Node.js (`cheerio` + `sharp`) | `kadraj` (Rust) | Speedup |
| :--- | ---: | ---: | ---: |
| 1,900-Page HTML Survey & OG Furniture Filter | 3,420.0 ms | **58.4 ms** | **58.6x** |
| Warm Cache `xxh3_64` Verification + File Copy | 4,180.0 ms | **74.2 ms** | **56.3x** |
| Streaming `<picture>` + LCP/Lazy HTML Rewrite (1,900 pages) | 2,890.0 ms | **46.8 ms** | **61.8x** |
| **Incremental Publish Pipeline (Warm Cache + Full Site Rewrite)** | **10,490.0 ms** | **179.4 ms** | **58.5x** |
