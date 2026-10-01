//! When output becomes visible: buffered by default, immediate with `--unbuffered`.

mod common;

use common::exec;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Starts jx, feeds it one line, and says whether its output shows up while
/// its input is still open.
fn output_while_input_is_open(flags: &[&str]) -> bool {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jx"))
        .args(flags)
        .args(["-c", ".a"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"a\":1}\n").unwrap();
    stdin.flush().unwrap();

    let (seen, lines) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    thread::spawn(move || {
        let mut line = String::new();
        if BufReader::new(stdout).read_line(&mut line).is_ok() && !line.is_empty() {
            let _ = seen.send(line);
        }
    });
    let early = lines.recv_timeout(Duration::from_millis(500)).is_ok();
    drop(stdin);
    child.wait().unwrap();
    early
}

#[test]
fn piped_output_is_held_back_until_the_run_ends() {
    assert!(!output_while_input_is_open(&[]));
}

#[test]
fn unbuffered_output_appears_at_once() {
    assert!(output_while_input_is_open(&["--unbuffered"]));
}

#[test]
fn buffering_never_changes_what_is_printed() {
    let input: String = (0..30_000).map(|n| format!("{{\"a\":{n}}}\n")).collect();
    let buffered = exec(&["-c", ".a"], &input);
    let unbuffered = exec(&["--unbuffered", "-c", ".a"], &input);
    assert_eq!(buffered.stdout, unbuffered.stdout);
    assert_eq!(buffered.stdout.lines().count(), 30_000);
}

#[test]
fn everything_is_written_before_the_exit_code_is_known() {
    let out = exec(&["-c", ".a"], "{\"a\":1}\n{\"a\":2}\n[3]\n{\"a\":4}\n");
    assert_eq!(out.stdout, "1\n2\n4\n");
    assert_eq!(out.code, 5);
}
