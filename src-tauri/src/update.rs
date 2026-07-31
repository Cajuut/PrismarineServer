use anyhow::{Context, Result};
use serde::Serialize;

pub const REPO_OWNER: &str = "Cajuut";
pub const REPO_NAME: &str = "PrismarineServer";

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub latest_version: String,
    pub current_version: String,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
    pub error: Option<String>,
}

fn parse_version(version: &str) -> Vec<u32> {
    version
        .trim_start_matches('v')
        .split('.')
        .filter_map(|p| p.parse::<u32>().ok())
        .collect()
}

/// Returns true if `newer` is a higher version than `older` (semver-ish comparison)
pub fn is_newer_version(newer: &str, older: &str) -> bool {
    let a = parse_version(newer);
    let b = parse_version(older);
    for (x, y) in a.iter().zip(b.iter()) {
        if x > y {
            return true;
        }
        if x < y {
            return false;
        }
    }
    a.len() > b.len()
}

/// Query the GitHub releases API for the latest release and look for a setup exe.
pub async fn check_for_updates(current_version: &str) -> Result<UpdateInfo> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        REPO_OWNER, REPO_NAME
    );

    let client = reqwest::Client::builder()
        .user_agent(format!("Prismarine/{}", current_version))
        .build()
        .context("Failed to build HTTP client")?;

    let resp = client.get(&url).send().await?;

    // 404 means no release yet
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateInfo {
            available: false,
            latest_version: current_version.to_string(),
            current_version: current_version.to_string(),
            download_url: None,
            release_notes: None,
            error: None,
        });
    }

    if !resp.status().is_success() {
        return Ok(UpdateInfo {
            available: false,
            latest_version: current_version.to_string(),
            current_version: current_version.to_string(),
            download_url: None,
            release_notes: None,
            error: Some(format!("GitHub API error: {}", resp.status())),
        });
    }

    let json: serde_json::Value = resp.json().await?;

    let tag = json["tag_name"].as_str().unwrap_or("").to_string();
    let notes = json["body"].as_str().map(|s| s.to_string());
    let assets = json["assets"].as_array().cloned().unwrap_or_default();

    // Look for the NSIS setup exe asset
    let download_url = assets.iter().find_map(|a| {
        let name = a["name"].as_str().unwrap_or("");
        let content_type = a["content_type"].as_str().unwrap_or("");
        let is_exe = name.ends_with(".exe") || content_type == "application/x-msdownload"
            || content_type == "application/octet-stream";
        let is_setup = name.contains("setup") || name.contains("Setup");
        if is_exe && is_setup {
            a["browser_download_url"].as_str().map(|s| s.to_string())
        } else {
            None
        }
    });

    let available = !tag.is_empty() && is_newer_version(&tag, current_version);

    Ok(UpdateInfo {
        available,
        latest_version: if tag.is_empty() {
            current_version.to_string()
        } else {
            tag
        },
        current_version: current_version.to_string(),
        download_url,
        release_notes: notes,
        error: None,
    })
}

/// Download the installer to a temp file and return its path.
pub async fn download_installer(url: &str) -> Result<std::path::PathBuf> {
    let client = reqwest::Client::builder()
        .user_agent("Prismarine-Updater")
        .build()
        .context("Failed to build HTTP client")?;

    let resp = client.get(url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;

    let temp_dir = std::env::temp_dir();
    let dest = temp_dir.join("Prismarine-Setup.exe");
    std::fs::write(&dest, &bytes).context("Failed to write installer to temp")?;

    Ok(dest)
}

/// Launch the installer silently, wait for this process to exit, then relaunch the app.
///
/// On Windows this writes a small batch script that:
///   1. Waits for Prismarine.exe to exit
///   2. Runs the NSIS installer with /S (silent)
///   3. Relaunches the installed app
pub fn install_and_relaunch(installer_path: &std::path::Path) -> Result<()> {
    let current_exe = std::env::current_exe().context("Failed to resolve current exe")?;
    let app_path = current_exe.display().to_string();
    let installer = installer_path.display().to_string();

    let batch = format!(
        "@echo off\r\n\
         echo Waiting for Prismarine to exit...\r\n\
         :wait\r\n\
         tasklist /FI \"IMAGENAME eq Prismarine.exe\" | find /I \"Prismarine.exe\" >nul\r\n\
         if not errorlevel 1 (\r\n\
             timeout /t 1 /nobreak >nul\r\n\
             goto wait\r\n\
         )\r\n\
         echo Installing update...\r\n\
         \"{}\" /S\r\n\
         echo Done. Starting Prismarine...\r\n\
         start \"\" \"{}\"\r\n\
         del \"%~f0\"\r\n",
        installer, app_path
    );

    let batch_path = std::env::temp_dir().join("Prismarine-Update.bat");
    std::fs::write(&batch_path, batch).context("Failed to write updater script")?;

    #[cfg(target_os = "windows")]
    {
        let _child = std::process::Command::new("cmd")
            .arg("/c")
            .arg(&batch_path)
            .spawn()
            .context("Failed to launch updater script")?;
        // Detach: let it run independently
        drop(_child);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = &batch_path;
        anyhow::bail!("Auto-update is currently only supported on Windows");
    }

    Ok(())
}
