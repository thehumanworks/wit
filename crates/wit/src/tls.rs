use std::sync::OnceLock;

/// `wit` links both rustls crypto backends through transitive deps, so
/// process startup must choose one before the first HTTPS client is built.
pub fn ensure_rustls_provider() {
    static INSTALLED: OnceLock<()> = OnceLock::new();

    INSTALLED.get_or_init(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
    });
}
