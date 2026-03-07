use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if let Err(err) = run() {
        panic!("failed to prepare embedded helm-apps asset: {err}");
    }
}

fn run() -> Result<(), String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|e| format!("OUT_DIR: {e}"))?);
    let dst = out_dir.join("helm-apps");
    println!("cargo:rerun-if-env-changed=HELM_APPS_CHART_PATH");
    println!("cargo:rerun-if-env-changed=HELM_APPS_GITHUB_REPO");
    println!("cargo:rerun-if-env-changed=HELM_APPS_GITHUB_REF");

    let src = if let Ok(path) = env::var("HELM_APPS_CHART_PATH") {
        PathBuf::from(path)
    } else {
        fetch_chart_from_github(&out_dir)?
    };

    if !src.join("Chart.yaml").exists() {
        return Err(format!(
            "source chart not found: {}",
            src.join("Chart.yaml").display()
        ));
    }

    emit_rerun_markers(&src).map_err(|e| format!("emit rerun markers: {e}"))?;
    copy_dir_replace(&src, &dst)
        .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    Ok(())
}

fn fetch_chart_from_github(out_dir: &Path) -> Result<PathBuf, String> {
    let repo = env::var("HELM_APPS_GITHUB_REPO")
        .unwrap_or_else(|_| "https://github.com/alvnukov/helm-apps.git".to_string());
    let reference =
        env::var("HELM_APPS_GITHUB_REF").unwrap_or_else(|_| "helm-apps-1.8.4".to_string());
    let checkout_dir = out_dir.join("helm-apps-checkout");
    if checkout_dir.exists() {
        fs::remove_dir_all(&checkout_dir)
            .map_err(|e| format!("remove stale checkout {}: {e}", checkout_dir.display()))?;
    }

    let status = Command::new("git")
        .args(["clone", "--depth", "1", "--branch", &reference, &repo])
        .arg(&checkout_dir)
        .status()
        .map_err(|e| format!("run git clone for helm-apps chart: {e}"))?;
    if !status.success() {
        return Err(format!("git clone failed for {} (ref {})", repo, reference));
    }

    let src = checkout_dir.join("charts/helm-apps");
    if !src.join("Chart.yaml").exists() {
        return Err(format!(
            "chart not found in cloned repo: {}",
            src.join("Chart.yaml").display()
        ));
    }
    Ok(src)
}

fn emit_rerun_markers(root: &Path) -> io::Result<()> {
    println!("cargo:rerun-if-changed={}", root.display());
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            emit_rerun_markers(&path)?;
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    Ok(())
}

fn copy_dir_replace(src: &Path, dst: &Path) -> io::Result<()> {
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;
    copy_dir_recursive(src, dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let out_path = dst.join(entry.file_name());
        if entry_path.is_dir() {
            fs::create_dir_all(&out_path)?;
            copy_dir_recursive(&entry_path, &out_path)?;
        } else {
            fs::copy(&entry_path, &out_path)?;
        }
    }
    Ok(())
}
