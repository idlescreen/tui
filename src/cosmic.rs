use std::process::Command;

pub fn is_cosmic_de() -> bool {
    for var in ["XDG_CURRENT_DESKTOP", "DESKTOP_SESSION"] {
        if std::env::var(var)
            .map(|v| v.to_lowercase().contains("cosmic"))
            .unwrap_or(false)
        {
            return true;
        }
    }
    std::path::Path::new("/usr/bin/cosmic-panel").exists()
}

pub fn is_cosmic_applet_installed() -> bool {
    std::path::Path::new("/usr/bin/idlescreen-applet").exists()
        || std::path::Path::new("/usr/bin/trance-applet").exists()
}

/// Whether the fingerprint-verified IdleScreen repository is configured.
///
/// `install.sh` is the only supported way to set this up: it verifies the
/// signing-key fingerprint before import and writes the source from pinned
/// content. This probe exists so the applet install below can refuse to run
/// against an unconfigured system instead of resolving `idle-cosmic` from
/// whatever repository happens to be configured for root.
fn repo_is_configured() -> bool {
    ["/etc/apt/sources.list.d/idlescreen.list", "/etc/yum.repos.d/idlescreen.repo"]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
}

pub fn install_cosmic_applet() -> Result<(), String> {
    let has_dnf = std::path::Path::new("/usr/bin/dnf").exists();
    let has_apt = std::path::Path::new("/usr/bin/apt").exists();

    // Fail closed. An earlier version of this path ran
    // `pkexec env IDLE_REQUIRE_MANIFEST_SIGNATURE=1 dnf|apt install` and the
    // comment claimed that made the transaction surface a missing-key error.
    // No package manager reads that variable — only `install.sh` and the daemon
    // do — so the "gate" did nothing, and because this path never configured
    // the repo, the install resolved the name from *any* configured repository,
    // as root.
    if !repo_is_configured() {
        return Err(
            "IdleScreen repository is not configured. Run install.sh first — it verifies the \
             signing key fingerprint before importing it."
                .to_string(),
        );
    }

    let status = if has_dnf {
        Command::new("pkexec")
            .args(["dnf", "install", "-y", "idle-cosmic"])
            .status()
    } else if has_apt {
        Command::new("pkexec")
            .args(["apt", "install", "-y", "idle-cosmic"])
            .status()
    } else {
        return Err("Error: No supported package manager (dnf/apt)".to_string());
    };

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("Installation exited with status: {}", s)),
        Err(e) => Err(format!("Failed to run installer: {}", e)),
    }
}
