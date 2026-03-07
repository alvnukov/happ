use std::io::Read;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct ChildOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum ChildWaitError {
    StdoutUnavailable,
    StderrUnavailable,
    StdoutLimitExceeded { limit: usize },
    StderrLimitExceeded { limit: usize },
    Timeout { timeout: Duration, stderr: Vec<u8> },
    WaitFailed { reason: String },
}

#[derive(Debug)]
struct PipeReadResult {
    data: Vec<u8>,
    truncated: bool,
}

pub(crate) fn wait_child_with_timeout_limited(
    mut child: Child,
    timeout: Duration,
    poll_interval: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<ChildOutput, ChildWaitError> {
    let Some(stdout) = child.stdout.take() else {
        return Err(ChildWaitError::StdoutUnavailable);
    };
    let Some(stderr) = child.stderr.take() else {
        return Err(ChildWaitError::StderrUnavailable);
    };
    let stdout_reader = spawn_pipe_reader_limited(stdout, max_stdout_bytes);
    let stderr_reader = spawn_pipe_reader_limited(stderr, max_stderr_bytes);
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = join_pipe_reader(stdout_reader);
                let stderr = join_pipe_reader(stderr_reader);
                if stdout.truncated {
                    return Err(ChildWaitError::StdoutLimitExceeded {
                        limit: max_stdout_bytes,
                    });
                }
                if stderr.truncated {
                    return Err(ChildWaitError::StderrLimitExceeded {
                        limit: max_stderr_bytes,
                    });
                }
                return Ok(ChildOutput {
                    status,
                    stdout: stdout.data,
                    stderr: stderr.data,
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = join_pipe_reader(stdout_reader);
                    let stderr = join_pipe_reader(stderr_reader);
                    return Err(ChildWaitError::Timeout {
                        timeout,
                        stderr: stderr.data,
                    });
                }
                thread::sleep(poll_interval);
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_pipe_reader(stdout_reader);
                let _ = join_pipe_reader(stderr_reader);
                return Err(ChildWaitError::WaitFailed {
                    reason: err.to_string(),
                });
            }
        }
    }
}

fn spawn_pipe_reader_limited<R>(mut pipe: R, max_bytes: usize) -> thread::JoinHandle<PipeReadResult>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        let mut truncated = false;
        loop {
            let Ok(n) = pipe.read(&mut buf) else {
                break;
            };
            if n == 0 {
                break;
            }
            if out.len() < max_bytes {
                let rem = max_bytes - out.len();
                let keep = rem.min(n);
                out.extend_from_slice(&buf[..keep]);
                if keep < n {
                    truncated = true;
                }
            } else {
                truncated = true;
            }
        }
        PipeReadResult {
            data: out,
            truncated,
        }
    })
}

fn join_pipe_reader(reader: thread::JoinHandle<PipeReadResult>) -> PipeReadResult {
    reader.join().unwrap_or(PipeReadResult {
        data: Vec::new(),
        truncated: false,
    })
}
