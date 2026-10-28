#[test]
fn shipped_binaries_reject_legacy_override_without_echoing_it() {
    for (binary, args) in [
        (
            env!("CARGO_BIN_EXE_wit"),
            vec!["--hostname", "invalid!", "search", "-q", "rust"],
        ),
        (
            env!("CARGO_BIN_EXE_wit-mcp"),
            vec!["--hostname", "invalid!"],
        ),
    ] {
        // Invalid hostname also prevents network access if the override guard regresses.
        let output = std::process::Command::new(binary)
            .args(args)
            .env(
                "WIT_GITHUB_API_URL",
                "https://synthetic-secret@proxy.invalid/custom",
            )
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("WIT_GITHUB_API_URL"), "{stderr}");
        assert!(stderr.contains("Unset it"), "{stderr}");
        assert!(!stderr.contains("synthetic-secret"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret"));
    }
}
