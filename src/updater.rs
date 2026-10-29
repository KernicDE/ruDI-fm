//! Self-update from the rolling GitHub release `latest` (one build per push to master).

use std::fs;
use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

const RELEASE_URL: &str = "https://api.github.com/repos/KernicDE/ruDI-fm/releases/tags/latest";

/// Commit of this build; set by CI, absent in local builds (no updates for those).
const BUILD: Option<&str> = option_env!("RUDI_BUILD");

const ASSET: Option<&str> = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("rudi-fm-linux-x86_64")
} else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
    Some("rudi-fm-windows-x86_64.exe")
} else if cfg!(target_os = "macos") {
    Some("rudi-fm-macos-universal")
} else {
    None
};

struct Release {
    commit: String,
    url: String,
}

fn agent(secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(secs)))
        .user_agent(concat!("ruDI.FM/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The newest release, if it differs from this build.
fn newer() -> Result<Option<Release>> {
    let (Some(build), Some(asset)) = (BUILD, ASSET) else { return Ok(None) };
    let v: Value = agent(10).get(RELEASE_URL).call()?.body_mut().read_json()?;
    let commit = v["body"].as_str().unwrap_or("").trim().to_string();
    if commit.is_empty() || commit == build {
        return Ok(None);
    }
    let url = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"] == asset))
        .and_then(|x| x["browser_download_url"].as_str())
        .with_context(|| format!("Release enthält kein Asset {asset}"))?
        .to_string();
    Ok(Some(Release { commit, url }))
}

/// Startup check: is a newer build available?
pub fn available() -> bool {
    matches!(newer(), Ok(Some(_)))
}

/// `rudi-fm update`
pub fn run() -> Result<()> {
    if BUILD.is_none() || ASSET.is_none() {
        bail!("Dieser Build unterstützt kein Self-Update (lokaler Build oder nicht unterstützte Plattform)");
    }
    let Some(rel) = newer()? else {
        println!("ruDI.FM ist aktuell.");
        return Ok(());
    };
    println!("Lade Update {} …", &rel.commit[..rel.commit.len().min(7)]);
    let bytes = agent(300).get(&rel.url).call()?.body_mut().with_config().limit(200 * 1024 * 1024).read_to_vec()?;

    let exe = std::env::current_exe()?;
    let new = exe.with_extension("new");
    let mut f = fs::File::create(&new).with_context(|| format!("{} nicht beschreibbar", new.display()))?;
    f.write_all(&bytes)?;
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new, fs::Permissions::from_mode(0o755))?;
    }
    // Windows cannot overwrite a running exe, but it can rename it.
    #[cfg(windows)]
    let old = {
        let old = exe.with_extension("old");
        let _ = fs::remove_file(&old);
        fs::rename(&exe, &old)?;
        old
    };
    if let Err(e) = fs::rename(&new, &exe) {
        #[cfg(windows)]
        let _ = fs::rename(&old, &exe);
        return Err(e).context("Binary ersetzen");
    }
    println!("Aktualisiert. Bitte ruDI.FM neu starten.");
    Ok(())
}
