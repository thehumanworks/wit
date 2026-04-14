use wit::ensure_rustls_provider;

#[test]
fn ensure_rustls_provider_installs_a_process_default() {
    ensure_rustls_provider();
    assert!(
        rustls::crypto::CryptoProvider::get_default().is_some(),
        "expected a process-wide rustls provider to be installed"
    );
}
