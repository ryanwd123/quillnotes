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

Notes:
- The first startup run is a cold start (129 ms). The other runs are warm.
