// Scratch directories for the unit tests: each one new, empty and unique (the process id and a counter name
// it, so tests running in parallel and parallel runs never share one), and removed when it's dropped, also
// when the test fails. Keep the guard until the test ends: `let dir = Scratch::new("name");`, not `let _ =`.
// Fake programs for a test to run are written with `write_executable`, and a test waits up to `WAIT` for
// something that should happen at once.

use std::fs;
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// How long a test waits for something that should happen at once (an event, a reply, a file). It only
/// bounds a failing test, so it's long: on a busy machine, a few seconds have run out before.
pub const WAIT: Duration = Duration::from_secs(30);

pub struct Scratch(PathBuf);

impl Scratch {
    /// An empty directory under the system temp dir, `briardr-<name>-<pid>-<n>`. Keep `name` short where a
    /// socket goes inside: a Unix socket path can't be much longer than 100 bytes.
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("briardr-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir); // left by an earlier run that had the same process id
        fs::create_dir_all(&dir).expect("create a scratch directory");
        Scratch(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes an executable file, a fake program for a test to run, at `path`. A child process writes it: a file
/// this process opens for writing is also open, for a moment, in each process another test thread starts
/// meanwhile (between its fork and exec), and on Linux running the file then fails with "Text file busy".
pub fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$0" && chmod 755 "$0""#])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("start sh to write an executable");
    let mut stdin = child.stdin.take().expect("sh's stdin");
    stdin
        .write_all(contents.as_bytes())
        .expect("write an executable");
    drop(stdin);
    let status = child.wait().expect("write an executable");
    assert!(status.success(), "writing {} failed", path.display());
}
