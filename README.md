# Paradox

Paradox is a zero-dependency Rust prototype for recording and comparing a
native program's standard I/O, plus an explicit event-replay library for
applications that add their own instrumentation.

The first release is deliberately a process-level replay harness. It does **not**
intercept Windows system calls, `RDTSC`, thread scheduling, memory writes, or CPU
instructions. It does not inject code into or sandbox a target. Consequently,
it cannot promise deterministic replay of arbitrary native binaries or
instruction-level reverse debugging.

## Supported environment

- Windows 11, x64
- Rust stable toolchain
- No third-party Rust crates

The code uses only Rust's standard library. It is kept portable enough to run
the unit tests on other development hosts, but Windows 11 x64 is the supported
target.

## Build and test

```powershell
cargo fmt --check
cargo test
cargo build --release
```

## Record and replay a process

Record a program by piping its standard input to Paradox. The target's stdout
and stderr are passed through, and a trace stores the input, outputs, executable
metadata, and exit status:

```powershell
"hello`n" | .\target\release\paradox.exe record --output run.trace -- .\target.exe --mode demo
```

Replay explicitly names the executable to run. The path in a trace is metadata;
Paradox will not launch it on its own:

```powershell
.\target\release\paradox.exe replay run --trace run.trace --executable .\target.exe
```

Replay runs the executable again. It can repeat file writes, network requests,
or other external side effects. Use only trusted traces and programs. Paradox
captures stdin/stdout/stderr and exit status, but not environment variables,
network responses, filesystem state, thread interleavings, or other system
inputs. A difference in any of those can cause divergence.

## Inspect and navigate a trace

```powershell
.\target\release\paradox.exe replay debug --trace run.trace
```

The trace debugger lists events and accepts `step-forward`, `step-back`,
`inspect-registers`, and `quit`. This navigates recorded event records; it does
not single-step CPU instructions. Register inspection reports that this trace
contains no register snapshots.

## Instrumented deterministic inputs

Applications can use the library's `EventSession` to log explicit
nondeterministic values and consume them again in the same order:

```rust
use paradox_replay::interceptor::EventSession;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = EventSession::record_to("events.trace")?;
    let captured = session.nondeterministic_value(b"application supplied input")?;
    assert_eq!(captured, b"application supplied input");
    session.finish()?;
    Ok(())
}
```

During replay, open the same file with `EventSession::replay_from`, call
`nondeterministic_value` at the corresponding points, and finish the session.
Replay returns recorded bytes instead of the caller's current bytes and reports
an error if the event kind or sequence does not match. This is an explicit
application API; it does not hook operating-system boundaries.

## Trace format, version 1

All integers are little-endian. A trace is:

| Part | Encoding |
| --- | --- |
| Header | 8-byte `PARADXTR` magic, `u16` version (`1`), `u16` flags (`0`) |
| Event frame | `u8` kind, 3 reserved zero bytes, `u64` sequence, `u32` payload length, payload, `u64` FNV-1a checksum |
| Trailer | 8-byte `PARADEND` marker, `u64` event count |

Event kinds are `1` process metadata, `2` stdin, `3` stdout, `4` stderr, `5`
exit status, and `32` explicitly instrumented application values. Payloads are
limited to 64 MiB. Frames are streamed through buffered file I/O and decoded
into owned values. Checksums detect accidental corruption; they do not provide
authentication or protect a trace against malicious modification. The current
format is not memory-mapped and does not offer zero-copy decoding.

## Architecture

- `storage`: bounded, sequenced trace frames and validation
- `recorder`: appends explicit events
- `interceptor`: application-level source/replay boundary
- `replay`: event cursor with forward and backward navigation
- `main`: process I/O capture, rerun-and-compare, and CLI

## Current limits and next engineering steps

True native time-travel debugging requires platform-specific instrumentation,
complete capture of nondeterministic inputs, coordinated thread recording, and
consistent memory/register snapshots. Those capabilities are not present in
this version. Before using this as a production debugger, the project needs
Windows-specific design and testing for process instrumentation, supported
target constraints, snapshot consistency, trace durability, and hostile or
malformed input handling. The present CLI should be treated as a local
development prototype, not a security boundary or a replacement for a native
debugger.
