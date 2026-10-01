//! Summarising collections: sorting, grouping, folding and counting.

mod common;

use common::{exec, run};

const ORDERS: &str = r#"[
  {"id":1,"customer":"ada","total":30,"items":2},
  {"id":2,"customer":"linus","total":10,"items":1},
  {"id":3,"customer":"ada","total":20,"items":5},
  {"id":4,"customer":"grace","total":10,"items":3}
]"#;

#[test]
fn sort_by_orders_by_one_or_several_keys() {
    assert_eq!(
        run("sort_by(.total) | map(.id)", ORDERS),
        "[2,4,3,1]\n",
        "ties keep their input order"
    );
    assert_eq!(run("sort_by(-.total) | map(.id)", ORDERS), "[1,3,2,4]\n");
    assert_eq!(
        run("sort_by(.total, .items) | map(.id)", ORDERS),
        "[2,4,3,1]\n"
    );
    assert_eq!(
        run("sort_by(.customer, -.total) | map(.id)", ORDERS),
        "[1,3,4,2]\n"
    );
}

#[test]
fn group_by_collects_equal_keys() {
    assert_eq!(
        run(
            "group_by(.customer) | map({customer: .[0].customer, orders: length, spent: (map(.total) | add)})",
            ORDERS
        ),
        "[{\"customer\":\"ada\",\"orders\":2,\"spent\":50},{\"customer\":\"grace\",\"orders\":1,\"spent\":10},{\"customer\":\"linus\",\"orders\":1,\"spent\":10}]\n"
    );
}

#[test]
fn unique_and_unique_by_remove_duplicates() {
    assert_eq!(
        run("map(.customer) | unique", ORDERS),
        "[\"ada\",\"grace\",\"linus\"]\n"
    );
    assert_eq!(run("unique_by(.total) | map(.id)", ORDERS), "[2,3,1]\n");
}

#[test]
fn min_max_and_their_by_variants() {
    assert_eq!(run("map(.total) | [min, max]", ORDERS), "[10,30]\n");
    assert_eq!(
        run("min_by(.total).id, max_by(.total).id", ORDERS),
        "2\n1\n"
    );
    assert_eq!(run("[] | [min, max]", "null"), "[null,null]\n");
}

#[test]
fn add_sums_numbers_concatenates_strings_arrays_and_objects() {
    assert_eq!(run("map(.total) | add", ORDERS), "70\n");
    assert_eq!(run("add", r#"["a","b","c"]"#), "\"abc\"\n");
    assert_eq!(run("add", "[[1],[2,3]]"), "[1,2,3]\n");
    assert_eq!(run("add", r#"[{"a":1},{"b":2}]"#), "{\"a\":1,\"b\":2}\n");
    assert_eq!(run("add", "[]"), "null\n");
}

#[test]
fn reduce_folds_a_stream_into_one_value() {
    assert_eq!(
        run("reduce .[] as $o ({}; .[$o.customer] += $o.total)", ORDERS),
        "{\"ada\":50,\"linus\":10,\"grace\":10}\n"
    );
    assert_eq!(run("reduce .[] as $o (0; . + $o.items)", ORDERS), "11\n");
    assert_eq!(run("reduce range(1; 6) as $n (1; . * $n)", "null"), "120\n");
}

#[test]
fn foreach_emits_the_running_state() {
    assert_eq!(
        run("[foreach .[] as $o (0; . + $o.total)]", ORDERS),
        "[30,40,60,70]\n"
    );
    assert_eq!(
        run(
            "[foreach .[] as $o (0; . + $o.total; select(. >= 50))]",
            ORDERS
        ),
        "[60,70]\n"
    );
    assert_eq!(
        run("[foreach range(5) as $i (null; $i; [$i, .])]", "null"),
        "[[0,0],[1,1],[2,2],[3,3],[4,4]]\n"
    );
}

#[test]
fn counting_and_membership() {
    assert_eq!(run("length", ORDERS), "4\n");
    assert_eq!(run("map(select(.total > 10)) | length", ORDERS), "2\n");
    assert_eq!(run("map(.customer) | index(\"grace\")", ORDERS), "3\n");
    assert_eq!(
        run("any(.[]; .total > 25), all(.[]; .total > 5)", ORDERS),
        "true\ntrue\n"
    );
    assert_eq!(
        run("map(.total) | any(. > 100), all(. > 5)", ORDERS),
        "false\ntrue\n"
    );
}

#[test]
fn slurp_aggregates_a_whole_stream() {
    let lines = "{\"v\":1}\n{\"v\":5}\n{\"v\":3}\n";
    assert_eq!(
        exec(
            &[
                "-s",
                "-c",
                "{n: length, max: (map(.v) | max), sum: (map(.v) | add)}"
            ],
            lines
        )
        .stdout,
        "{\"n\":3,\"max\":5,\"sum\":9}\n"
    );
    assert_eq!(
        exec(&["-n", "-c", "reduce inputs as $r (0; . + $r.v)"], lines).stdout,
        "9\n"
    );
}

#[test]
fn bsearch_finds_positions_in_sorted_arrays() {
    assert_eq!(
        run(
            "[bsearch(1), bsearch(3), bsearch(0), bsearch(9)]",
            "[1,3,5,7]"
        ),
        "[0,1,-1,-5]\n"
    );
}
