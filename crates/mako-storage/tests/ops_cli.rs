use std::{fs, process::Command};

use mako_storage::{VolumeState, read_volume_marker};
use tempfile::TempDir;

#[test]
fn provision_and_fence_require_confirmation_and_support_dry_run() {
    let root = TempDir::new().expect("root");
    let volume = root.path().join("volume");
    fs::create_dir(&volume).expect("volume");
    let executable = env!("CARGO_BIN_EXE_mako-storage-ops");
    let base = [
        format!("--database-path={}", volume.display()),
        "--service=mako-data-plane".into(),
        "--database-id=mako-data-plane-cli-test".into(),
    ];

    let dry_run = Command::new(executable)
        .arg("provision")
        .args(&base)
        .arg("--dry-run")
        .output()
        .expect("dry run");
    assert!(dry_run.status.success());
    assert!(String::from_utf8_lossy(&dry_run.stdout).contains("dry-run: provision"));
    assert_eq!(fs::read_dir(&volume).expect("empty").count(), 0);

    let missing_confirmation = Command::new(executable)
        .arg("provision")
        .args(&base)
        .output()
        .expect("missing confirmation");
    assert!(!missing_confirmation.status.success());
    assert!(String::from_utf8_lossy(&missing_confirmation.stderr).contains("--confirm=PROVISION"));

    assert!(
        Command::new(executable)
            .arg("provision")
            .args(&base)
            .arg("--confirm=PROVISION")
            .status()
            .expect("provision")
            .success()
    );
    assert_eq!(
        read_volume_marker(&volume).expect("marker").state,
        VolumeState::Provisioned
    );

    assert!(
        Command::new(executable)
            .arg("qualify-recovery")
            .args(&base)
            .args([
                "--project=prj_qualification",
                "--environment=env_qualification",
                "--confirm=QUALIFY_RECOVERY",
            ])
            .status()
            .expect("initialize nonempty volume")
            .success()
    );

    let missing_confirmation = Command::new(executable)
        .arg("fence")
        .args(&base)
        .output()
        .expect("missing confirmation");
    assert!(!missing_confirmation.status.success());
    assert!(String::from_utf8_lossy(&missing_confirmation.stderr).contains("--confirm=FENCE"));
    assert!(
        Command::new(executable)
            .arg("fence")
            .args(&base)
            .arg("--confirm=FENCE")
            .status()
            .expect("fence")
            .success()
    );
    assert_eq!(
        read_volume_marker(&volume).expect("marker").state,
        VolumeState::Fenced
    );

    let missing_confirmation = Command::new(executable)
        .arg("activate")
        .args(&base)
        .output()
        .expect("missing activation confirmation");
    assert!(!missing_confirmation.status.success());
    assert!(String::from_utf8_lossy(&missing_confirmation.stderr).contains("--confirm=ACTIVATE"));
    assert!(
        Command::new(executable)
            .arg("activate")
            .args(&base)
            .arg("--confirm=ACTIVATE")
            .status()
            .expect("activate")
            .success()
    );
    assert_eq!(
        read_volume_marker(&volume).expect("marker").state,
        VolumeState::Active
    );
}
