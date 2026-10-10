#![allow(clippy::large_futures)]

//! Verifier integration test: a cell result longer than the output cap is
//! clipped by characters. A fake runtime answers one cell with a long
//! non-ASCII repr, so the byte at the cap lands inside a character.

#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use pa_core::kernel::manager::{KernelStartOptions, ReplKernelManager};
use pa_core::kernel::shared::{
    ExecuteOptions, ExecuteStatus, HostRequestHandlers, KernelManagerOptions, KernelShutdownOptions,
};

const FAKE_RUNTIME: &str = r#"#!/usr/bin/env python3
import json
import sys

print(json.dumps({"event": "ready", "protocol": 3, "python": "3.13.0"}), flush=True)
for line in sys.stdin:
    try:
        req = json.loads(line)
    except Exception:
        continue
    if req.get("code") == "wide-result":
        text = "'" + "é" * 70000 + "'"
        sys.stdout.write(json.dumps({"event": "result", "id": req.get("id"), "text": text}) + "\n")
    sys.stdout.write(json.dumps({"event": "done", "id": req.get("id"), "status": "ok"}) + "\n")
    sys.stdout.flush()
"#;

fn fake_runtime_path() -> std::path::PathBuf {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("fake-kernel");
    std::fs::write(&path, FAKE_RUNTIME).expect("write fake runtime");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // Leak the temp dir: the script must exist until the spawned child exits.
    std::mem::forget(dir);
    path
}

#[tokio::test]
async fn a_wide_result_past_the_cap_is_clipped_and_the_kernel_keeps_serving() {
    let manager = ReplKernelManager::new(KernelManagerOptions {
        python: Some(fake_runtime_path()),
        cwd: Some(std::env::temp_dir()),
        env: HashMap::new(),
        session_id: Some("result-clip-test".to_string()),
        host_handlers: HostRequestHandlers::new(),
        python_skills: Vec::new(),
        snapshot: None,
        bootstrap_code: None,
        stderr_log_path: None,
        on_background_work_settled: None,
    });
    manager
        .start(KernelStartOptions::default())
        .await
        .expect("fake kernel must start");

    let wide = tokio::time::timeout(
        Duration::from_secs(10),
        manager.execute("wide-result", ExecuteOptions::default()),
    )
    .await
    .expect("the cell must settle")
    .expect("execute must not fail");
    let expected = format!(
        "'{}\n[... output truncated at 65536 chars ...]",
        "\u{e9}".repeat(65_535)
    );
    assert_eq!(
        (wide.status, wide.result.as_deref()),
        (ExecuteStatus::Ok, Some(expected.as_str()))
    );

    let follow = tokio::time::timeout(
        Duration::from_secs(10),
        manager.execute("42", ExecuteOptions::default()),
    )
    .await
    .expect("the follow-up cell must settle")
    .expect("follow-up execute must not fail");
    assert_eq!(follow.status, ExecuteStatus::Ok);

    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        manager.shutdown(KernelShutdownOptions::default()),
    )
    .await;
}
