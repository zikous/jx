//! Event streams: `--stream`, `tostream` and `fromstream`.

mod common;

use common::{exec, run_args as run};

const DOC: &str = r#"{"users":[{"id":1,"tags":["a","b"]},{"id":2,"tags":[]}]}"#;

#[test]
fn stream_emits_leaf_events_and_closing_events() {
    assert_eq!(
        run(&["-c", "--stream", "."], r#"{"a":[1,2],"b":3}"#),
        "[[\"a\",0],1]\n[[\"a\",1],2]\n[[\"a\",1]]\n[[\"b\"],3]\n[[\"b\"]]\n"
    );
    assert_eq!(run(&["-c", "--stream", "."], "3"), "[[],3]\n");
    assert_eq!(run(&["-c", "--stream", "."], "[]"), "[[],[]]\n");
}

#[test]
fn stream_events_can_be_filtered_to_leaves() {
    let leaves = run(&["-c", "--stream", "select(length == 2)"], DOC);
    assert_eq!(
        leaves,
        "[[\"users\",0,\"id\"],1]\n[[\"users\",0,\"tags\",0],\"a\"]\n[[\"users\",0,\"tags\",1],\"b\"]\n[[\"users\",1,\"id\"],2]\n[[\"users\",1,\"tags\"],[]]\n"
    );
}

#[test]
fn fromstream_rebuilds_values() {
    assert_eq!(
        run(&["-c", "--stream", "-n", "fromstream(inputs)"], DOC),
        format!("{DOC}\n")
    );
    assert_eq!(
        run(&["-c", "fromstream(tostream)"], DOC),
        format!("{DOC}\n")
    );
}

#[test]
fn truncate_stream_drops_leading_path_elements() {
    assert_eq!(
        run(
            &[
                "-c",
                "[1 | truncate_stream([[0],1],[[1,0],2],[[1,0]],[[1]])]"
            ],
            "null"
        ),
        "[[[0],2],[[0]]]\n"
    );
}

#[test]
fn tostream_of_a_scalar_and_containers() {
    assert_eq!(
        run(&["-c", "[tostream]"], "{\"a\":{\"b\":2}}"),
        "[[[\"a\",\"b\"],2],[[\"a\",\"b\"]],[[\"a\"]]]\n"
    );
}

#[test]
fn a_truncated_document_still_yields_the_events_before_the_error() {
    let out = exec(&["-c", "--stream", "."], r#"{"a":[1,"#);
    assert_eq!(out.stdout, "[[\"a\",0],1]\n");
    assert_eq!(out.code, 5);
    assert!(
        out.stderr.contains("Unfinished JSON term"),
        "{}",
        out.stderr
    );
}
