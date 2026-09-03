//! Testing seam for running external commands (currently: `nvidia-smi` for
//! the discrete GPU, since NitroControl uses it as a subprocess fallback
//! rather than linking NVML directly — see docs/architecture.md).

use std::io;

/// Runs a command and returns its captured stdout.
pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<String>;
}

/// Runs real subprocesses. Used in production.
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<String> {
        use std::io::Read;
        use std::process::Stdio;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        // docs/architecture.md ("Sensor polling..."): no sensor read may
        // block indefinitely — a hung subprocess (e.g. a stuck `nvidia-smi`)
        // must time out and surface as an ordinary read error (mapped to
        // `Unknown` by callers), never freeze the calling thread.
        const TIMEOUT: Duration = Duration::from_secs(3);
        const POLL_INTERVAL: Duration = Duration::from_millis(20);

        let mut child = std::process::Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .spawn()?;

        // Drain stdout on a background thread so the child can never block
        // on a full pipe buffer while we poll `try_wait` below.
        let mut stdout_pipe = child.stdout.take();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = stdout_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });

        let start = Instant::now();
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if start.elapsed() >= TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("`{program}` did not exit within {TIMEOUT:?}"),
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        let buf = rx.recv_timeout(TIMEOUT).unwrap_or_default();
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockCommandRunner {
        responses: Mutex<HashMap<String, io::Result<String>>>,
    }

    fn clone_result(result: &io::Result<String>) -> io::Result<String> {
        match result {
            Ok(s) => Ok(s.clone()),
            Err(e) => Err(io::Error::from(e.kind())),
        }
    }

    impl MockCommandRunner {
        pub fn new() -> Self {
            Self::default()
        }

        fn key(program: &str, args: &[&str]) -> String {
            format!("{program} {}", args.join(" "))
        }

        pub fn set_output(&self, program: &str, args: &[&str], stdout: impl Into<String>) {
            self.responses
                .lock()
                .unwrap()
                .insert(Self::key(program, args), Ok(stdout.into()));
        }

        pub fn set_not_found(&self, program: &str, args: &[&str]) {
            self.responses.lock().unwrap().insert(
                Self::key(program, args),
                Err(io::Error::from(io::ErrorKind::NotFound)),
            );
        }
    }

    impl CommandRunner for MockCommandRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<String> {
            self.responses
                .lock()
                .unwrap()
                .get(&Self::key(program, args))
                .map(clone_result)
                .unwrap_or_else(|| Err(io::Error::from(io::ErrorKind::NotFound)))
        }
    }

    #[cfg(test)]
    mod mock_tests {
        use super::*;

        #[test]
        fn returns_configured_output() {
            let mock = MockCommandRunner::new();
            mock.set_output("nvidia-smi", &["--query"], "47, 12\n");
            assert_eq!(mock.run("nvidia-smi", &["--query"]).unwrap(), "47, 12\n");
        }

        #[test]
        fn unconfigured_command_is_not_found() {
            let mock = MockCommandRunner::new();
            let err = mock.run("nvidia-smi", &["--query"]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn explicit_not_found_overrides_default() {
            let mock = MockCommandRunner::new();
            mock.set_not_found("nvidia-smi", &["--query"]);
            let err = mock.run("nvidia-smi", &["--query"]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_runner_captures_stdout() {
        let runner = RealCommandRunner;
        let output = runner.run("echo", &["hello"]).unwrap();
        assert_eq!(output.trim(), "hello");
    }

    #[test]
    fn real_runner_reports_missing_binary() {
        let runner = RealCommandRunner;
        let err = runner
            .run("nitroctl-definitely-not-a-real-binary", &[])
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
