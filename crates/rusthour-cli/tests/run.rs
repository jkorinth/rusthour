//! The `run` command's output, as the M1 trial and scripts consume it.

use std::process::Command;

fn run(args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_rusthour"))
        .arg("run")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn run_prints_the_state_hash_and_the_counts() {
    let a = run(&["--steps", "20"]);
    assert_eq!(a["protocol_version"], 2);
    assert_eq!(a["tick"], 20);
    assert!(a["state_hash"].is_u64());
    assert_eq!(a["vehicles"], 0);
    assert_eq!(a["waiting"], 0);
    assert_eq!(a, run(&["--steps", "20"]), "same seed, same output");
}

#[test]
fn a_missing_map_is_a_clear_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_rusthour"))
        .args(["run", "--map", "/nonexistent.rhnet"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read /nonexistent.rhnet"));
}

#[test]
fn vehicles_without_a_map_is_an_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_rusthour"))
        .args(["run", "--vehicles", "5"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--map"));
}
