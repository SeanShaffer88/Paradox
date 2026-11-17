# Paradox

You are an expert systems software engineer and low-level architect. I want you to design, structure, and implement a production-grade, zero-dependency, high-performance **Lightweight, Deterministic Time-Travel Replay Debugger** for native binaries (targeting Windows platforms). 

The goal of this repository is to achieve legendary open-source status by solving one of the most painful problems in systems development: debugging non-deterministic bugs (race conditions, intermittent memory corruption, and asynchronous timing failures) without heavy hypervisor overhead.

Please generate a comprehensive, fully functional codebase and architectural blueprint following the specifications below.

---

### Core Architecture & Engineering Requirements

#### 1. The Two-Phase Model: Record & Replay
* **Recorder Engine:** Intercepts non-deterministic inputs at the OS/environment boundary. It must capture syscall return values, high-resolution timer reads (`RDTSC`), thread scheduling yields, and unmanaged memory state deltas.
* **Replay Engine:** Feeds pre-recorded trace logs back into the target binary under a controlled sandbox, overriding real-world OS calls and thread yields with deterministic pre-recorded payloads.
* **Bi-Directional Time Control:** Implement lightweight memory checkpoints (snapshots of heap and register states) at regular interval thresholds, enabling forward and backward instruction stepping by restoring the nearest snapshot and re-running the trace log delta.

#### 2. High-Performance Storage Layer
* **Lock-Free Ring Buffer:** Use memory-mapped files (`mmap`) and a lock-free circular buffer to stream trace logs asynchronously with near-zero latency impact on the running process.
* **Zero-Copy Serialization:** Design compact binary log structures using zero-copy deserialization to keep trace overhead minimal.

#### 3. Tech Stack & Engineering Standards
* **Language:** Written entirely in **Rust** for bare-metal performance, fearless concurrency, and absolute memory safety without garbage collection pauses.
* **Code Quality:** Production-ready, heavily documented, modular, and idiomatic Rust code with clean separation of concerns (`recorder`, `replay`, `interceptor`, `storage`, and `cli`).
* **CLI Interface:** Provide a robust command-line interface supporting commands such as:
  * `replay record --output trace.log ./target_binary.exe`
  * `replay debug --trace trace.log`
  * Step commands (`step-back`, `step-forward`, `inspect-registers`)

---

### Deliverables Expected from You

1. **Project Directory Structure:** A clean workspace layout (`Cargo.toml`, `src/main.rs`, `src/recorder.rs`, `src/replay.rs`, `src/interceptor.rs`, `src/storage.rs`).
2. **Complete Source Code:** Full, working implementations for each module—avoid placeholders or `// TODO` comments for core logic.
3. **Architecture Documentation (`README.md`):** A comprehensive guide explaining the internal mechanics, binary format specs, limitations, and how developers can build and test the tool locally.
