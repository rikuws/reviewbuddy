use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 1_048_576;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: Vec<u8>,
    pub stderr_bytes: Vec<u8>,
    pub timed_out: bool,
    pub duration_ms: u128,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct CommandRunner {
    program: String,
    args: Vec<String>,
    working_directory: Option<PathBuf>,
    timeout: Duration,
    output_limit_bytes: usize,
}

impl CommandRunner {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_directory: None,
            timeout: DEFAULT_TIMEOUT,
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, path: impl AsRef<Path>) -> Self {
        self.working_directory = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn output_limit_bytes(mut self, limit: usize) -> Self {
        self.output_limit_bytes = limit;
        self
    }

    pub fn run(self) -> Result<CommandOutput, String> {
        let started_at = Instant::now();
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(working_directory) = &self.working_directory {
            command.current_dir(working_directory);
        }

        let mut child = command.spawn().map_err(|error| {
            format!(
                "Failed to launch {}{}: {error}",
                self.program,
                working_directory_suffix(self.working_directory.as_deref())
            )
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let output_limit = self.output_limit_bytes;
        let stdout_reader =
            stdout.map(|stream| thread::spawn(move || read_limited(stream, output_limit)));
        let stderr_reader =
            stderr.map(|stream| thread::spawn(move || read_limited(stream, output_limit)));

        let mut timed_out = false;
        let exit_code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => {
                    if started_at.elapsed() >= self.timeout {
                        timed_out = true;
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("Failed to poll {}: {error}", self.program));
                }
            }
        };

        let stdout = join_reader(stdout_reader)?;
        let stderr = join_reader(stderr_reader)?;

        let stdout_text = String::from_utf8_lossy(&stdout.bytes).trim().to_string();
        let stderr_text = String::from_utf8_lossy(&stderr.bytes).trim().to_string();

        Ok(CommandOutput {
            exit_code,
            stdout: stdout_text,
            stderr: stderr_text,
            stdout_bytes: stdout.bytes,
            stderr_bytes: stderr.bytes,
            timed_out,
            duration_ms: started_at.elapsed().as_millis(),
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        })
    }
}

#[derive(Debug, Default)]
struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_limited(mut stream: impl Read, limit: usize) -> LimitedOutput {
    let mut output = LimitedOutput::default();
    let mut buffer = [0_u8; 8192];

    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        let remaining = limit.saturating_sub(output.bytes.len());
        if remaining == 0 {
            output.truncated = true;
            continue;
        }
        let to_copy = read.min(remaining);
        output.bytes.extend_from_slice(&buffer[..to_copy]);
        if to_copy < read {
            output.truncated = true;
        }
    }

    output
}

fn join_reader(handle: Option<thread::JoinHandle<LimitedOutput>>) -> Result<LimitedOutput, String> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| "Failed to join command output reader.".to_string()),
        None => Ok(LimitedOutput::default()),
    }
}

fn working_directory_suffix(path: Option<&Path>) -> String {
    path.map(|path| format!(" in {}", path.display()))
        .unwrap_or_default()
}
