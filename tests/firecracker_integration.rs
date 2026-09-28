use sandy::{FirecrackerConfig, FirecrackerService};

#[tokio::test]
async fn firecracker_executes_python_when_enabled() {
    if std::env::var("SANDY_FC_TEST").as_deref() != Ok("1") {
        return;
    }

    let service = FirecrackerService::with_defaults().expect("valid Firecracker config");
    assert!(
        service.is_available(),
        "SANDY_FC_TEST=1 requires Firecracker, KVM, kernel, and rootfs"
    );
    let result = service
        .execute_python_code("print(6 * 7)")
        .await
        .expect("microVM execution succeeds");
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("42"));
}

#[test]
fn default_paths_can_be_overridden() {
    let config = FirecrackerConfig {
        kernel_path: "/tmp/test-vmlinux".into(),
        rootfs_path: "/tmp/test-rootfs.ext4".into(),
        ..Default::default()
    };
    assert_eq!(
        config.kernel_path,
        std::path::PathBuf::from("/tmp/test-vmlinux")
    );
    assert_eq!(
        config.rootfs_path,
        std::path::PathBuf::from("/tmp/test-rootfs.ext4")
    );
}
