# Release Metrics

One row per release, measured on the release build with `scripts/measure.ps1`.
The script opens `sample.md` with throwaway settings folders.

- **Startup:** time from process launch until the main window exists and the message loop is idle. Median of 7 runs (min–max).
- **Private / Working set:** memory about 3 s after startup, with `sample.md` open.
- **Idle CPU:** CPU time used during 10 s of idle time with the window open. Windows counts CPU time in 15.6 ms ticks, so 15.6 ms means one tick or less.

Machine: Intel i7-13700K, 64 GB RAM, Windows 11 Pro 22631.

| Version | Date | Commit | Binary size | Startup (median, min–max) | Private | Working set | Idle CPU / 10 s | Threads | Handles |
|---------|------|--------|-------------|---------------------------|---------|-------------|-----------------|---------|---------|
| 0.2.0 | 2026-11-07 | `84f5a44` | 991,232 B (968 KiB) | 55.9 ms (52.5–129.0) | 17.6 MB | 37.6 MB | 15.6 ms | 17 | 280 |
| 0.3.0 | 2026-11-14 | `0332c4d` | 1,001,472 B (978 KiB) | 43.4 ms (42.1–137.0) | 17.1 MB | 34.9 MB | 15.6 ms | 20 | 278 |
| 0.4.0 | 2026-11-22 | `fee7d24` | 1,028,608 B (1,004 KiB) | 54.9 ms (50.3–138.4) | 16.7 MB | 36.9 MB | 0.0 ms | 18 | 280 |

Notes:
- The first startup run is a cold start (129 ms in 0.2.0, 137 ms in 0.3.0, 138 ms in 0.4.0). The other runs are warm.
- 0.4.0 startup is not a regression. Rebuilt from the v0.3.0 tag and measured back to back on the same day (15 runs each), 0.3.0 took 61.6 ms and 0.4.0 took 63.0 ms. The machine was slower that day than on 2026-11-14.
- Thread counts move between runs (16–23 in the same session) because the OS creates thread-pool workers on demand, so small changes there don't mean anything.
