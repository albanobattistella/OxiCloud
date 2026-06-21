# Thumbnail performance — Phase 0 baseline

> **Phase 1.1 (shrink-on-load) is now merged — see "Phase 1.1 results" at the
> bottom for the before/after.** The tables below remain the Phase 0 baseline
> (the "before").

The "before" numbers every later phase must beat. Captured on **14 cores** with
the current `image` 0.25 pipeline (`render_thumbnail_from_data` /
`render_all_thumbnails_from_data` in
`src/infrastructure/services/thumbnail_service.rs`).

> Heap = logical allocation high-water mark (counting allocator), not RSS.
> The synthetic corpus is high-entropy (gradient + noise), so JPEG sizes and
> decode work are realistic-to-slightly-pessimistic. Drop real photos into
> `benches/corpus/` (same filenames) to re-baseline on real data.

## Reproduce

```bash
# Peak RAM + saturated throughput (Task 0.3) → target/bench-baseline-fase0.json
cargo run --release --features bench --example bench_thumbnails_mem

# Per-size latency + output bytes (Task 0.2) → target/criterion/report/index.html
cargo bench --features bench           # do NOT pipe through `tail` — it truncates the log;
                                       # results are saved under target/criterion/ regardless
```

## A. Per-image — peak heap, single-thread latency, output size

| case             | fmt  |   source  |   MP | render_all ms | peak heap MB | out KB (3 sizes) |
|------------------|------|-----------|-----:|--------------:|-------------:|-----------------:|
| jpeg_12mp        | jpeg | 4000×3000 | 12.0 |        111.30 |         96.1 |               57 |
| jpeg_24mp        | jpeg | 6000×4000 | 24.0 |        207.23 |        151.0 |               41 |
| jpeg_48mp        | jpeg | 8000×6000 | 48.0 |        397.67 |        260.9 |               38 |
| jpeg_exif_orient | jpeg | 4000×3000 | 12.0 |        121.82 |        107.6 |               59 |
| png_large        | png  | 3000×2000 |  6.0 |         34.36 |         58.4 |               63 |
| webp_large       | webp | 1280×853  |  1.1 |         21.05 |         20.7 |              113 |
| gif_large        | gif  | 600×600   |  0.4 |         12.86 |         15.4 |              232 |
| small_300        | jpeg | 300×300   |  0.1 |          9.94 |          8.0 |              184 |

## B. Saturated throughput (14 threads, 3 s window)

| case      |   source  |   MP | photos/sec | eff ms/photo |
|-----------|-----------|-----:|-----------:|-------------:|
| jpeg_12mp | 4000×3000 | 12.0 |       44.4 |        22.52 |
| jpeg_24mp | 6000×4000 | 24.0 |       25.3 |        39.51 |
| jpeg_48mp | 8000×6000 | 48.0 |       12.4 |        80.49 |

Scaling is sub-linear (14 threads ≈ 4.9× single-thread): memory-bandwidth bound
(moving 96–261 MB per decode) + rayon oversubscription (each caller thread fans
3 sizes onto the shared rayon pool).

## C. Per-size latency — criterion median ms (one size in isolation vs all-three)

| case             | Icon ms | Preview ms | Large ms | all-3 ms | Large/all |
|------------------|--------:|-----------:|---------:|---------:|----------:|
| jpeg_12mp        |   75.42 |      97.17 |   106.30 |   107.11 |     99.3% |
| jpeg_24mp        |  148.65 |     190.05 |   200.70 |   203.44 |     98.7% |
| jpeg_48mp        |  303.31 |     375.33 |   386.04 |   391.24 |     98.7% |
| jpeg_exif_orient |   90.68 |     122.57 |   119.27 |   119.33 |    100.0% |
| png_large        |   13.32 |      25.87 |    32.26 |    32.59 |     99.0% |
| webp_large       |   15.83 |      19.51 |    24.47 |    24.45 |    100.1% |
| gif_large        |    2.28 |       5.15 |    12.94 |    12.92 |    100.2% |
| small_300        |    0.85 |       3.22 |     9.94 |     9.92 |    100.2% |

## Key findings (these steer Phase 1)

1. **Decode dominates: 70–99 % of total time.** For jpeg_12mp, rendering all
   three sizes (107 ms) costs barely more than rendering Icon alone (75 ms) —
   the full-resolution decode is the shared cost; per-size resize+encode is
   cheap on top. ⇒ **Shrink-on-load (Task 1.1) is the single biggest lever**,
   bigger than first estimated.

2. **Peak heap scales linearly with megapixels** (~2× the RGBA bitmap):
   12 MP→96 MB, 48 MP→261 MB. With the real `cpus/2` semaphore that is up to
   7×261 MB ≈ 1.8 GB on a 48 MP burst — the OOM ceiling that caps concurrency.
   Shrink-on-load collapses this ~16× and unlocks Task 1.5 (raise the semaphore).

3. **Task 2.1 "defer Large" is now DROPPED — the benchmark refutes it.**
   Because all three sizes share one decode (Large/all ≈ 99 %), deferring Large
   saves ~9 ms eager but forces a *second full decode* (~106 ms) when the
   lightbox opens — it roughly **doubles** total decode work. Keep generating
   all sizes in one pass.

4. **No-upscale (Task 1.4) confirmed minor:** small_300's Large (9.9 ms)
   upscales 300→800; clamping recovers a few ms and avoids artefacts.

5. **PNG/GIF/WebP get no DCT shrink-on-load** — only `fast_image_resize`
   (Task 1.2) speeds their resize portion.

---

# Phase 1.1 results — shrink-on-load (DCT scale-on-decode for JPEG)

Implemented via `jpeg-decoder` in `decode_oriented` / `decode_jpeg_scaled`
(`src/infrastructure/services/thumbnail_service.rs`). The JPEG decoder now emits
the image at the smallest DCT scale (1/8·1/4·1/2·1/1) whose long axis is still ≥
the largest needed thumbnail (800 px), so the full-resolution bitmap is never
materialised. Non-JPEG and unusual JPEG colour spaces fall back to a full decode.
Same machine (14 cores), same corpus.

### Latency — `render_all`, single thread (ms)

| case      | before | after  | speedup |
|-----------|-------:|-------:|--------:|
| jpeg_12mp | 111.30 |  60.64 |  1.84×  |
| jpeg_24mp | 207.23 | 113.71 |  1.82×  |
| jpeg_48mp | 397.67 | 202.88 |  1.96×  |
| jpeg_exif | 121.82 |  60.12 |  2.03×  |
| png_large |  34.36 |  33.67 |  ~1×  (no DCT, expected) |

### Peak heap per decode (MB) — the headline win

| case      | before | after | reduction |
|-----------|-------:|------:|----------:|
| jpeg_12mp |   96.1 |  17.6 |   5.5×  |
| jpeg_24mp |  151.0 |  24.9 |   6.1×  |
| jpeg_48mp |  260.9 |  17.6 |  14.8×  |
| jpeg_exif |  107.6 |  18.9 |   5.7×  |

Peak heap is now **decoupled from source resolution** (~18–25 MB regardless of
MP — bounded by the 800 px decode, not the original). 48 MP now uses *less* than
24 MP because it hits the 1/8 scale (1000×750) vs 24 MP's 1/4 (1500×1000).

### Saturated throughput (14 threads, photos/sec)

| case      | before | after | speedup |
|-----------|-------:|------:|--------:|
| jpeg_12mp |   44.4 | 140.8 |  3.17×  |
| jpeg_24mp |   25.3 |  74.7 |  2.95×  |
| jpeg_48mp |   12.4 |  45.3 |  3.65×  |

Throughput improved **more** than single-thread latency (3.2× vs 1.8× at 12 MP):
parallel efficiency rose from ~4.9× to ~8.5× across 14 threads because the 16×
smaller decode buffers relieve the memory-bandwidth ceiling.

### Quality gate — shrink-on-load vs full decode (Preview 400 px)

| case      | SSIM   | PSNR dB |
|-----------|-------:|--------:|
| jpeg_12mp | 0.9875 |   47.42 |
| jpeg_24mp | 0.9927 |   48.91 |
| jpeg_48mp | 0.9939 |   49.37 |
| small_300 | 0.9995 |   55.17 |

All **SSIM ≥ 0.98** (acceptance criterion met) and PSNR 47–55 dB (>40 dB =
visually indistinguishable). Output bytes unchanged (e.g. 12 MP: 57→58 KB).

### Follow-ups this unlocked
- **Task 1.5** (raise `cpus/2` → `cpus`): peak heap no longer scales with MP, so
  the OOM ceiling that justified halving concurrency is largely gone. ✅ done below.
- The `MAX_DECODE_PIXELS` 50 MP reject could be relaxed — huge JPEGs now decode
  cheaply at 1/8 — but that is a behaviour change, deferred.

---

# Phase 1.5 results — raise decode-concurrency cap (`cpus/2` → `cpus`)

`max_concurrent_decodes()` now defaults to all cores (was half), overridable via
`OXICLOUD_THUMBNAIL_DECODE_CONCURRENCY`. Safe only because Phase 1.1 decoupled
peak heap from source resolution.

Measured with a harness that mirrors the **real service path** (Table D:
`tokio::Semaphore(permits)` + `spawn_blocking`, many concurrent requests),
14 cores, 3 s window:

| case      | 7 permits (`cpus/2`, old) | 14 permits (`cpus`, new) | 28 (`cpus*2`) |
|-----------|--------------------------:|-------------------------:|--------------:|
| jpeg_12mp |                      92.7 |          **133.7 (1.44×)** | 133.3 (—) |
| jpeg_24mp |                      49.5 |           **69.9 (1.41×)** | 71.1 (+1.7%) |

- **~1.4× throughput** on the real path, for free; peak heap unchanged (17–25 MB).
- `cpus*2` yields nothing → `cpus` is the right ceiling for CPU-bound work.
- It's 1.4× not 2× because `render_all` fans its 3 sizes onto rayon, so 7 permits
  already partly fill all cores — the remaining headroom is **Task 1.7**
  (rayon oversubscription).

---

# Phase 1.2 results — SIMD resize (`fast_image_resize`, Lanczos3)

Replaced the `image` crate's scalar resampler with `fast_image_resize`
(AVX2/SSE4.1/NEON) in the shared `encode_thumbnail` helper; `render_all` now
converts to RGB8 once and SIMD-resizes the shared buffer per size. Lanczos3 for
downscaling, CatmullRom when upscaling (avoids Lanczos ringing). Also folded the
duplicated path-variant `generate_all_sizes_background` into the shared render
path, so it too gets shrink-on-load + SIMD. "before" = post-1.5 state.

### Single-thread latency `render_all` (ms) and peak heap (MB)

| case      | ms before | ms after | speedup | heap before | heap after |
|-----------|----------:|---------:|--------:|------------:|-----------:|
| jpeg_12mp |     60.89 |    56.46 |  1.08×  |        17.6 |    **7.1** |
| jpeg_24mp |    113.98 |   106.76 |  1.07×  |        24.9 |   **10.1** |
| jpeg_48mp |    203.29 |   198.75 |  1.02×  |        17.6 |    **7.1** |
| **png_large** | 33.63 | **12.92** | **2.60×** |    58.4 |   **27.1** |
| gif_large |     12.93 |     7.99 |  1.62×  |        15.4 |        5.9 |
| webp_large|     21.15 |    16.89 |  1.25×  |        20.7 |        8.3 |
| small_300 |     10.64 |     6.37 |  1.67×  |         8.0 |        3.9 |

- **JPEG: only ~6–8 %** — shrink-on-load already shrank the decoded bitmap, so
  the resize was a small slice of the time. But **peak heap fell another ~2.5×**
  (7 MB): fir works on tight RGB buffers with no intermediate `DynamicImage`,
  and RGB conversion now happens once instead of per size.
- **PNG: 2.6×** (and GIF/WebP 1.25–1.6×) — exactly as predicted: these decode at
  full resolution (no DCT shrink), so the SIMD resize dominates the win.

### Throughput (≈ +10–15 %, run-to-run noisy)

Saturated 12 MP ≈ 142→157 photos/s; semaphore-bounded 12 MP @ 14 permits
≈ 134→148. Directionally up; treat as noise-bounded.

### Quality gate (vs full-decode CatmullRom at identical dims)

| case      | SSIM   | PSNR dB |
|-----------|-------:|--------:|
| jpeg_12mp | 0.9865 |   47.21 |
| jpeg_24mp | 0.9923 |   48.73 |
| jpeg_48mp | 0.9938 |   49.33 |
| small_300 | 0.9921 |   42.59 |

All **≥ 0.98**. (The upscale case `small_300` needed the Lanczos3→CatmullRom
upscale rule — Lanczos rings when enlarging; it was 0.954 before that fix.)

### Note
Output thumbnails are now exactly `max_dim` on the long side (e.g. 400×266),
vs the old `image::resize` fit-within which produced 399×266 — a ≤1 px change,
invisible under the frontend's `object-fit: cover`.

---

# Phase 1.7 — TESTED AND REVERTED (rayon oversubscription)

Hypothesis: `render_all`'s internal `par_iter` over the 3 sizes oversubscribes
the global rayon pool under load (cpus×3 tasks), capping burst throughput.
Tested by resizing the 3 sizes **sequentially** (parallelism across images
only). Measured before/after on 14 cores, two runs each:

| metric                       | before (par_iter) | after (sequential) | verdict |
|------------------------------|------------------:|-------------------:|---------|
| **PNG single-image latency** |          12.9 ms  |       **21.3 ms**  | **−66 % WORSE** |
| JPEG 12 MP single-image      |          57.8 ms  |          58.5 ms   | neutral |
| Saturated 12 MP (photos/s)   |            155.8  |        170 / 158   | flat (noise) |
| Semaphore @14 12 MP          |            157.7  |        164 / 157   | flat |
| Semaphore @28 12 MP          |            160.4  |        162 / 159   | flat |

**Verdict: reverted.** Throughput at the real operating point (14 permits) is
flat — rayon oversubscription was **not** the bottleneck. The "1.4× not 2×" of
Phase 1.5 is the single-threaded JPEG decode (which dominates post-shrink) plus
memory bandwidth, not rayon scheduling; even 28 concurrent renders (84 rayon
tasks) show no thrash. Meanwhile full-decode formats (PNG) regressed 66 % on
single-image latency because their resize-from-full-resolution genuinely
benefits from the per-image parallelism. Net negative → kept `par_iter`.

(Another "measure before believing" result, like the dropped Task 2.1.)

