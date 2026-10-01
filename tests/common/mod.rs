//! Helpers shared by the integration tests: each runs the jx binary.
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Runs jx with these arguments and standard input.
pub fn exec(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jx"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let data = stdin.to_owned();
    let feeder = std::thread::spawn(move || {
        let _ = input.write_all(data.as_bytes());
    });
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    Output {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

/// Runs jx with these arguments and returns its output, which must succeed.
pub fn run_args(args: &[&str], stdin: &str) -> String {
    let out = exec(args, stdin);
    assert_eq!(out.code, 0, "{args:?}: {}", out.stderr);
    out.stdout
}

/// Runs a program with compact output.
pub fn run(program: &str, input: &str) -> String {
    run_args(&["-c", program], input)
}

/// Writes a file under the test's temporary directory and returns its path.
pub fn fixture(dir: &str, name: &str, contents: &str) -> String {
    let dir = format!("{}/{dir}", env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(&dir).unwrap();
    let path = format!("{dir}/{name}");
    fs::write(&path, contents).unwrap();
    path
}
