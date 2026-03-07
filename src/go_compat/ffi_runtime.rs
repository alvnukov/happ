use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn env_bool(name: &str) -> bool {
    let Ok(raw) = env::var(name) else {
        return false;
    };
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub(crate) fn env_u64_or(name: &str, default: u64) -> u64 {
    let Ok(raw) = env::var(name) else {
        return default;
    };
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(crate) fn env_usize_or(name: &str, default: usize) -> usize {
    let Ok(raw) = env::var(name) else {
        return default;
    };
    raw.trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(crate) fn cache_dir_from_env(env_name: &str, default_dir_name: &str) -> PathBuf {
    if let Ok(raw) = env::var(env_name) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    env::temp_dir().join(default_dir_name)
}

pub(crate) fn helper_binary_name(base_name: &str) -> String {
    if cfg!(windows) {
        format!("{base_name}.exe")
    } else {
        base_name.to_string()
    }
}

pub(crate) fn fnv1a64(input: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in input {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(unix)]
pub(crate) fn set_executable_permissions_if_needed(path: &Path) -> std::io::Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
pub(crate) fn set_executable_permissions_if_needed(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
