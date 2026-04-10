use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELM_IR_HELPER_ZSTD_LEVEL: i32 = 19;

fn main() {
    if let Err(err) = run() {
        eprintln!("build asset preparation failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed=HAPP_GO_BIN");
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|e| format!("OUT_DIR: {e}"))?);
    prepare_embedded_helm_apps_chart(&out_dir)?;
    prepare_embedded_helm_ir_helper(&out_dir)?;
    Ok(())
}

fn prepare_embedded_helm_apps_chart(out_dir: &Path) -> Result<(), String> {
    let dst = out_dir.join("helm-apps");
    println!("cargo:rerun-if-env-changed=HELM_APPS_CHART_PATH");
    println!("cargo:rerun-if-env-changed=HELM_APPS_GITHUB_REPO");
    println!("cargo:rerun-if-env-changed=HELM_APPS_GITHUB_REF");

    let src = if let Ok(path) = env::var("HELM_APPS_CHART_PATH") {
        PathBuf::from(path)
    } else {
        fetch_chart_from_github(out_dir)?
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

fn prepare_embedded_helm_ir_helper(out_dir: &Path) -> Result<(), String> {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").map_err(|e| format!("CARGO_MANIFEST_DIR: {e}"))?,
    );
    let helper_dir = manifest_dir.join("src/go_compat/helm_ir_ffi_helper");
    let helper_main = helper_dir.join("main.go");
    let helper_mod = helper_dir.join("go.mod");
    let helper_sum = helper_dir.join("go.sum");
    let helper_vendor = helper_dir.join("vendor");
    if !helper_main.exists() {
        return Err(format!(
            "missing helm ir helper source: {}",
            helper_main.display()
        ));
    }
    if !helper_mod.exists() {
        return Err(format!(
            "missing helm ir helper go.mod: {}",
            helper_mod.display()
        ));
    }
    if !helper_vendor.is_dir() {
        return Err(format!(
            "missing helm ir helper vendor directory: {} (run `go mod vendor` in helper dir)",
            helper_vendor.display()
        ));
    }
    println!("cargo:rerun-if-changed={}", helper_main.display());
    println!("cargo:rerun-if-changed={}", helper_mod.display());
    if helper_sum.exists() {
        println!("cargo:rerun-if-changed={}", helper_sum.display());
    }
    println!("cargo:rerun-if-changed={}", helper_vendor.display());

    let target_os = env::var("CARGO_CFG_TARGET_OS").map_err(|e| format!("target os: {e}"))?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").map_err(|e| format!("target arch: {e}"))?;
    let go_os = map_go_os(&target_os)
        .ok_or_else(|| format!("unsupported target os for helm ir helper: {target_os}"))?;
    let go_arch = map_go_arch(&target_arch)
        .ok_or_else(|| format!("unsupported target arch for helm ir helper: {target_arch}"))?;

    let helper_out = out_dir.join(helper_binary_name_for_target(&target_os));
    let go_bin = env::var("HAPP_GO_BIN").unwrap_or_else(|_| "go".to_string());
    let output = Command::new(&go_bin)
        .current_dir(&helper_dir)
        .env("CGO_ENABLED", "0")
        .env("GOOS", go_os)
        .env("GOARCH", go_arch)
        .arg("build")
        .arg("-mod=vendor")
        .arg("-trimpath")
        .arg("-o")
        .arg(&helper_out)
        .arg("./main.go")
        .output()
        .map_err(|e| format!("run {go_bin} build for helm ir helper: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("helm ir helper build failed with status {}", output.status)
        } else {
            format!(
                "helm ir helper build failed with status {}: {}",
                output.status, stderr
            )
        });
    }
    if !helper_out.exists() {
        return Err(format!(
            "helm ir helper output is missing after build: {}",
            helper_out.display()
        ));
    }
    let helper_out_zstd =
        out_dir.join(format!("{}.zst", helper_binary_name_for_target(&target_os)));
    compress_file_zstd(&helper_out, &helper_out_zstd)?;
    if !helper_out_zstd.exists() {
        return Err(format!(
            "compressed helm ir helper output is missing after build: {}",
            helper_out_zstd.display()
        ));
    }
    println!(
        "cargo:rustc-env=HAPP_HELM_IR_HELPER_BIN_ZSTD={}",
        helper_out_zstd.display()
    );
    Ok(())
}

fn compress_file_zstd(src: &Path, dst: &Path) -> Result<(), String> {
    let mut input =
        fs::File::open(src).map_err(|e| format!("open helper binary for compression: {e}"))?;
    let mut output = fs::File::create(dst)
        .map_err(|e| format!("create compressed helper binary {}: {e}", dst.display()))?;
    zstd::stream::copy_encode(&mut input, &mut output, HELM_IR_HELPER_ZSTD_LEVEL).map_err(|e| {
        format!(
            "compress helper binary {} -> {}: {e}",
            src.display(),
            dst.display()
        )
    })
}

fn map_go_os(target_os: &str) -> Option<&'static str> {
    match target_os {
        "linux" => Some("linux"),
        "macos" => Some("darwin"),
        "windows" => Some("windows"),
        _ => None,
    }
}

fn map_go_arch(target_arch: &str) -> Option<&'static str> {
    match target_arch {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        "x86" => Some("386"),
        _ => None,
    }
}

fn helper_binary_name_for_target(target_os: &str) -> &'static str {
    if target_os == "windows" {
        "happ-helm-ir-ffi.exe"
    } else {
        "happ-helm-ir-ffi"
    }
}

fn fetch_chart_from_github(out_dir: &Path) -> Result<PathBuf, String> {
    let repo = env::var("HELM_APPS_GITHUB_REPO")
        .unwrap_or_else(|_| "https://github.com/alvnukov/helm-apps.git".to_string());
    let reference =
        env::var("HELM_APPS_GITHUB_REF").unwrap_or_else(|_| "helm-apps-1.8.11".to_string());
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
