use paradox_replay::recorder::Recorder;
use paradox_replay::replay::ReplayCursor;
use paradox_replay::storage::read_trace;
use paradox_replay::{Error, EventKind, TraceEvent};
use std::env;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
struct ProcessMetadata {
    executable: String,
    working_directory: String,
    arguments: Vec<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("paradox: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        print_help();
        return Ok(());
    };
    match command.to_string_lossy().as_ref() {
        "record" => record_command(args.collect()),
        "replay" => {
            let Some(subcommand) = args.next() else {
                return Err(Error::Process(
                    "expected `replay run` or `replay debug`".into(),
                ));
            };
            match subcommand.to_string_lossy().as_ref() {
                "run" => replay_command(args.collect()),
                "debug" => debug_command(args.collect()),
                _ => Err(Error::Process(
                    "expected `replay run` or `replay debug`".into(),
                )),
            }
        }
        "debug" => debug_command(args.collect()),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(Error::Process(format!(
            "unknown command `{other}`; use `paradox help`"
        ))),
    }
}

fn record_command(args: Vec<OsString>) -> Result<(), Error> {
    let (trace_path, executable, arguments) = parse_target_args(args)?;
    let working_directory = env::current_dir()?;
    let stdin = read_bounded(io::stdin(), MAX_CAPTURE_BYTES, "stdin")?;
    let output = run_child(&executable, &arguments, &working_directory, &stdin)?;
    let executable_text = executable
        .to_str()
        .ok_or_else(|| Error::Process("executable path must be valid UTF-8".into()))?;
    let cwd_text = working_directory
        .to_str()
        .ok_or_else(|| Error::Process("working directory must be valid UTF-8".into()))?;
    let argument_text = arguments
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::Process("arguments must be valid UTF-8".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut recorder = Recorder::create(&trace_path)?;
    recorder.record(
        EventKind::Process,
        &encode_metadata(&ProcessMetadata {
            executable: executable_text.to_owned(),
            working_directory: cwd_text.to_owned(),
            arguments: argument_text,
        }),
    )?;
    recorder.record(EventKind::Stdin, &stdin)?;
    recorder.record(EventKind::Stdout, &output.stdout)?;
    recorder.record(EventKind::Stderr, &output.stderr)?;
    recorder.record(
        EventKind::Exit,
        &encode_exit(output.status.code(), output.status.success()),
    )?;
    recorder.finish()?;

    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;
    println!("\nRecorded process result to {}.", trace_path.display());
    if let Some(code) = output.status.code() {
        if code != 0 {
            return Err(Error::Process(format!("target exited with code {code}")));
        }
    } else {
        return Err(Error::Process(
            "target terminated without an exit code".into(),
        ));
    }
    Ok(())
}

fn replay_command(args: Vec<OsString>) -> Result<(), Error> {
    let (trace_path, executable_override) = parse_replay_args(args)?;
    let events = read_trace(&trace_path)?;
    let recorded = ProcessRun::from_events(&events)?;
    let executable = executable_override;
    eprintln!(
        "Re-running {}. Replay can repeat the program's external side effects.",
        executable.display()
    );
    let arguments = recorded
        .metadata
        .arguments
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let working_directory = PathBuf::from(&recorded.metadata.working_directory);
    let actual = run_child(&executable, &arguments, &working_directory, &recorded.stdin)?;
    io::stdout().write_all(&actual.stdout)?;
    io::stderr().write_all(&actual.stderr)?;
    let mut differences = Vec::new();
    compare_stream("stdout", &recorded.stdout, &actual.stdout, &mut differences);
    compare_stream("stderr", &recorded.stderr, &actual.stderr, &mut differences);
    if actual.status.code() != recorded.exit_code || actual.status.success() != recorded.success {
        differences.push(format!(
            "exit status recorded {:?} (success: {}), replayed {:?} (success: {})",
            recorded.exit_code,
            recorded.success,
            actual.status.code(),
            actual.status.success()
        ));
    }
    if differences.is_empty() {
        println!("\nReplay matched the recorded stdout, stderr, and exit status.");
        Ok(())
    } else {
        Err(Error::Process(format!(
            "replay diverged: {}",
            differences.join("; ")
        )))
    }
}

fn debug_command(args: Vec<OsString>) -> Result<(), Error> {
    let path = parse_trace_arg(args)?;
    let events = read_trace(&path)?;
    println!("{} events in {}.", events.len(), path.display());
    for event in &events {
        println!(
            "#{:04} {:<8} {} bytes",
            event.sequence,
            event_name(event.kind),
            event.payload.len()
        );
    }
    println!("Commands: step-forward, step-back, inspect-registers, quit");
    let mut cursor = ReplayCursor::open(&path)?;
    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("paradox[{}]> ", cursor.position());
        io::stdout().flush()?;
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        match line.trim() {
            "step-forward" | "f" => match cursor.step_forward() {
                Some(event) => print_event(event),
                None => println!("At end of trace."),
            },
            "step-back" | "b" => match cursor.step_back() {
                Some(event) => print_event(event),
                None => println!("At start of trace."),
            },
            "inspect-registers" | "r" => {
                println!("No CPU register snapshots exist in this process-I/O trace.")
            }
            "quit" | "q" | "exit" => break,
            "" => {}
            _ => println!(
                "Unknown command. Use step-forward, step-back, inspect-registers, or quit."
            ),
        }
    }
    Ok(())
}

fn parse_target_args(args: Vec<OsString>) -> Result<(PathBuf, OsString, Vec<OsString>), Error> {
    let mut trace = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--output" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| Error::Process("--output needs a path".into()))?;
            trace = Some(PathBuf::from(value));
            index += 1;
        } else if args[index] == "--" {
            index += 1;
            break;
        } else {
            break;
        }
    }
    let trace = trace.ok_or_else(|| {
        Error::Process("usage: paradox record --output trace.log -- program.exe [args...]".into())
    })?;
    let executable = args
        .get(index)
        .ok_or_else(|| Error::Process("missing target executable".into()))?
        .clone();
    let arguments = args[index + 1..].to_vec();
    Ok((trace, executable, arguments))
}

fn parse_trace_arg(args: Vec<OsString>) -> Result<PathBuf, Error> {
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        if argument == "--trace" {
            return args
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| Error::Process("--trace needs a path".into()));
        }
    }
    Err(Error::Process(
        "usage: paradox replay debug --trace trace.log".into(),
    ))
}

fn parse_replay_args(args: Vec<OsString>) -> Result<(PathBuf, OsString), Error> {
    let mut trace = None;
    let mut executable = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        if argument == "--trace" {
            trace = Some(
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| Error::Process("--trace needs a path".into()))?,
            );
        } else if argument == "--executable" {
            executable = Some(
                args.next()
                    .ok_or_else(|| Error::Process("--executable needs a path".into()))?,
            );
        }
    }
    let trace = trace.ok_or_else(|| {
        Error::Process(
            "usage: paradox replay run --trace trace.log --executable program.exe".into(),
        )
    })?;
    let executable = executable.ok_or_else(|| {
        Error::Process(
            "replay requires an explicit --executable path; traces never choose a program to run"
                .into(),
        )
    })?;
    Ok((trace, executable))
}

fn run_child(
    executable: &OsString,
    arguments: &[OsString],
    working_directory: &Path,
    stdin_bytes: &[u8],
) -> Result<Output, Error> {
    let mut child = Command::new(executable)
        .args(arguments)
        .current_dir(working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::Process(format!("could not launch target: {error}")))?;
    let mut child_stdin = child.stdin.take().expect("piped stdin");
    let input = stdin_bytes.to_vec();
    let writer = thread::spawn(move || child_stdin.write_all(&input));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| Error::Process("stdin writer thread panicked".into()))??;
    Ok(output)
}

fn read_bounded<R: Read>(reader: R, limit: usize, name: &str) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::Process(format!("{name} exceeds {limit} bytes")));
    }
    Ok(bytes)
}

fn encode_metadata(metadata: &ProcessMetadata) -> Vec<u8> {
    let mut output = Vec::new();
    put_string(&mut output, &metadata.executable);
    put_string(&mut output, &metadata.working_directory);
    output.extend_from_slice(&(metadata.arguments.len() as u32).to_le_bytes());
    for argument in &metadata.arguments {
        put_string(&mut output, argument);
    }
    output
}

fn encode_exit(code: Option<i32>, success: bool) -> Vec<u8> {
    let mut output = Vec::with_capacity(5);
    output.push(u8::from(success));
    output.extend_from_slice(&code.unwrap_or(i32::MIN).to_le_bytes());
    output
}

fn compare_stream(name: &str, recorded: &[u8], replayed: &[u8], differences: &mut Vec<String>) {
    let Some(offset) = first_difference(recorded, replayed) else {
        return;
    };
    let detail = if offset < recorded.len().min(replayed.len()) {
        format!("first differs at byte {offset}")
    } else {
        format!("first differs at byte {offset} (one stream ended)")
    };
    differences.push(format!(
        "{name} {detail} (recorded {} bytes, replayed {} bytes)",
        recorded.len(),
        replayed.len()
    ));
}

fn first_difference(recorded: &[u8], replayed: &[u8]) -> Option<usize> {
    recorded
        .iter()
        .zip(replayed)
        .position(|(expected, actual)| expected != actual)
        .or_else(|| {
            (recorded.len() != replayed.len()).then_some(recorded.len().min(replayed.len()))
        })
}

fn put_string(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u32).to_le_bytes());
    output.extend_from_slice(value.as_bytes());
}

struct ProcessRun {
    metadata: ProcessMetadata,
    stdin: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    success: bool,
}

impl ProcessRun {
    fn from_events(events: &[TraceEvent]) -> Result<Self, Error> {
        let get = |index: usize, kind: EventKind| -> Result<&[u8], Error> {
            let event = events
                .get(index)
                .ok_or(Error::MissingEvent("process event"))?;
            if event.kind != kind {
                return Err(Error::UnexpectedEvent {
                    expected: kind as u8,
                    actual: event.kind as u8,
                });
            }
            Ok(&event.payload)
        };
        let metadata = decode_metadata(get(0, EventKind::Process)?)?;
        let stdin = get(1, EventKind::Stdin)?.to_vec();
        let stdout = get(2, EventKind::Stdout)?.to_vec();
        let stderr = get(3, EventKind::Stderr)?.to_vec();
        let exit = get(4, EventKind::Exit)?;
        if exit.len() != 5 {
            return Err(Error::InvalidTrace("invalid exit event length".into()));
        }
        let success = exit[0] != 0;
        let code = i32::from_le_bytes(exit[1..5].try_into().expect("checked length"));
        if events.len() != 5 {
            return Err(Error::InvalidTrace(
                "process trace must contain exactly five events".into(),
            ));
        }
        Ok(Self {
            metadata,
            stdin,
            stdout,
            stderr,
            exit_code: (code != i32::MIN).then_some(code),
            success,
        })
    }
}

fn decode_metadata(bytes: &[u8]) -> Result<ProcessMetadata, Error> {
    let mut reader = SliceReader { bytes, offset: 0 };
    let executable = reader.string()?;
    let working_directory = reader.string()?;
    let count = reader.u32()? as usize;
    if count > 65_536 {
        return Err(Error::InvalidTrace("too many process arguments".into()));
    }
    let mut arguments = Vec::with_capacity(count);
    for _ in 0..count {
        arguments.push(reader.string()?);
    }
    if reader.offset != bytes.len() {
        return Err(Error::InvalidTrace(
            "unexpected bytes in process metadata".into(),
        ));
    }
    Ok(ProcessMetadata {
        executable,
        working_directory,
        arguments,
    })
}

struct SliceReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl SliceReader<'_> {
    fn u32(&mut self) -> Result<u32, Error> {
        let end = self
            .offset
            .checked_add(4)
            .ok_or_else(|| Error::InvalidTrace("metadata overflow".into()))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| Error::InvalidTrace("truncated metadata".into()))?;
        self.offset = end;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("checked length"),
        ))
    }

    fn string(&mut self) -> Result<String, Error> {
        let length = self.u32()? as usize;
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| Error::InvalidTrace("metadata overflow".into()))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| Error::InvalidTrace("truncated metadata string".into()))?;
        self.offset = end;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::InvalidTrace("metadata is not UTF-8".into()))
    }
}

fn event_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Process => "process",
        EventKind::Stdin => "stdin",
        EventKind::Stdout => "stdout",
        EventKind::Stderr => "stderr",
        EventKind::Exit => "exit",
        EventKind::User => "user",
    }
}

fn print_event(event: &TraceEvent) {
    println!(
        "#{:04} {} ({} bytes)",
        event.sequence,
        event_name(event.kind),
        event.payload.len()
    );
    match event.kind {
        EventKind::Process => println!("  process launch metadata"),
        EventKind::Stdin => println!("  captured input (not displayed)"),
        EventKind::Stdout => println!("  captured stdout (not displayed)"),
        EventKind::Stderr => println!("  captured stderr (not displayed)"),
        EventKind::Exit => println!("  recorded process exit status"),
        EventKind::User => println!("  explicitly instrumented application event"),
    }
}

fn print_help() {
    println!(
        "Paradox — process I/O replay and trace inspection\n\
         \n\
         Commands:\n\
           paradox record --output trace.log -- program.exe [args...]\n\
           paradox replay run --trace trace.log --executable program.exe\n\
           paradox replay debug --trace trace.log\n\
           paradox debug --trace trace.log\n\
         \n\
         Record captures stdin, stdout, stderr, and exit status. Replay runs\n\
         the executable again; it does not intercept OS calls or instruction\n\
         execution. Replay can repeat external side effects."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_first_stream_mismatch_including_length_only_differences() {
        assert_eq!(first_difference(b"abc", b"abx"), Some(2));
        assert_eq!(first_difference(b"abc", b"abc!"), Some(3));
        assert_eq!(first_difference(b"same", b"same"), None);
    }

    #[test]
    fn metadata_round_trip() {
        let metadata = ProcessMetadata {
            executable: "sample.exe".into(),
            working_directory: "C:\\work".into(),
            arguments: vec!["--mode".into(), "test case".into()],
        };
        let decoded = decode_metadata(&encode_metadata(&metadata)).expect("decode");
        assert_eq!(decoded.executable, metadata.executable);
        assert_eq!(decoded.working_directory, metadata.working_directory);
        assert_eq!(decoded.arguments, metadata.arguments);
    }
}
