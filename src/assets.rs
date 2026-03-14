use include_dir::{include_dir, Dir, DirEntry};
use serde::Deserialize;
use std::fs;
use std::io;
use std::path::Path;

static EMBEDDED_HELM_APPS: Dir<'_> = include_dir!("$OUT_DIR/helm-apps");

pub fn has_helm_apps_chart() -> bool {
    EMBEDDED_HELM_APPS.get_file("Chart.yaml").is_some()
}

pub fn extract_helm_apps_chart(dst: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(dst)?;
    write_dir(&EMBEDDED_HELM_APPS, dst)
}

pub fn embedded_helm_apps_version() -> Option<String> {
    #[derive(Deserialize)]
    struct ChartMetadata {
        version: Option<String>,
    }

    let chart_yaml = EMBEDDED_HELM_APPS.get_file("Chart.yaml")?;
    let parsed: ChartMetadata = serde_yaml::from_slice(chart_yaml.contents()).ok()?;
    parsed
        .version
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_dir(dir: &Dir<'_>, dst: &Path) -> Result<(), io::Error> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(child) => {
                let next = dst.join(child.path().file_name().unwrap_or_default());
                fs::create_dir_all(&next)?;
                write_dir(child, &next)?;
            }
            DirEntry::File(file) => {
                let target = dst.join(file.path().file_name().unwrap_or_default());
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(target, file.contents())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_chart_is_available() {
        assert!(
            has_helm_apps_chart(),
            "embedded helm-apps Chart.yaml not found"
        );
    }

    #[test]
    fn embedded_chart_version_is_available() {
        let version = embedded_helm_apps_version().expect("embedded chart version");
        assert!(!version.trim().is_empty(), "embedded chart version is empty");
    }

    #[test]
    fn extract_helm_apps_chart_writes_chart_yaml() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        extract_helm_apps_chart(temp_dir.path()).expect("extract embedded chart");
        let chart_yaml_path = temp_dir.path().join("Chart.yaml");
        assert!(chart_yaml_path.exists(), "Chart.yaml not extracted");
        let chart_yaml = fs::read_to_string(chart_yaml_path).expect("read Chart.yaml");
        let version = embedded_helm_apps_version().expect("embedded version");
        assert!(
            chart_yaml.contains(&format!("version: {version}")),
            "extracted Chart.yaml does not contain embedded version"
        );
    }
}
