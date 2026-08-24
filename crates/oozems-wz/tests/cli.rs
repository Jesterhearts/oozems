use std::process::Command;

#[test]
fn argument_errors_are_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_oozems-wz"))
        .arg("--not-an-option")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("unexpected argument")
    );
}
