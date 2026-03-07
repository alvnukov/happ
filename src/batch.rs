use crate::cli::{BatchArgs, ImportArgs};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchChartJob {
    pub name: String,
    pub source_path: String,
    pub out_chart_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchFailure {
    pub chart: String,
    pub source_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReport {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub failures: Vec<BatchFailure>,
    pub jobs: Vec<BatchChartJob>,
}

pub fn run_batch_convert<F>(args: &BatchArgs, mut convert_chart: F) -> Result<BatchReport, String>
where
    F: FnMut(&ImportArgs) -> Result<(), String>,
{
    let jobs = discover_batch_jobs(args)?;
    fs::create_dir_all(&args.out_dir)
        .map_err(|e| format!("create output dir '{}': {e}", args.out_dir))?;

    let mut failures = Vec::new();
    let mut succeeded = 0usize;

    for job in &jobs {
        let mut import = ImportArgs::from_shared(job.source_path.clone(), &args.import);
        import.chart_name = Some(job.name.clone());
        import.out_chart_dir = Some(job.out_chart_dir.clone());
        match convert_chart(&import) {
            Ok(()) => succeeded += 1,
            Err(reason) => {
                failures.push(BatchFailure {
                    chart: job.name.clone(),
                    source_path: job.source_path.clone(),
                    reason,
                });
                if !args.keep_going {
                    break;
                }
            }
        }
    }

    let report = BatchReport {
        total: jobs.len(),
        succeeded,
        failed: failures.len(),
        failures,
        jobs,
    };

    if report.failed > 0 {
        let mut lines = vec![format!(
            "batch convert finished with {} error(s): succeeded={}, failed={}",
            report.failed, report.succeeded, report.failed
        )];
        for failure in &report.failures {
            lines.push(format!(
                "- {} ({}): {}",
                failure.chart, failure.source_path, failure.reason
            ));
        }
        return Err(lines.join("\n"));
    }
    Ok(report)
}

fn discover_batch_jobs(args: &BatchArgs) -> Result<Vec<BatchChartJob>, String> {
    let charts_dir = Path::new(&args.charts_dir);
    if !charts_dir.exists() {
        return Err(format!("charts dir '{}' does not exist", args.charts_dir));
    }
    if !charts_dir.is_dir() {
        return Err(format!(
            "charts dir '{}' is not a directory",
            args.charts_dir
        ));
    }

    let mut candidates = Vec::new();
    let entries = fs::read_dir(charts_dir)
        .map_err(|e| format!("read charts dir '{}': {e}", args.charts_dir))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("read charts dir entry: {e}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("read file type '{}': {e}", path.to_string_lossy()))?;
        if file_type.is_dir() {
            if path.join("Chart.yaml").is_file() {
                candidates.push(path);
            }
            continue;
        }
        if file_type.is_file() && is_chart_archive(&path) {
            candidates.push(path);
        }
    }

    if candidates.is_empty() {
        return Err(format!(
            "no charts found in '{}': expected chart directories with Chart.yaml or .tgz/.tar.gz archives",
            args.charts_dir
        ));
    }

    candidates.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    let mut used_names = BTreeMap::<String, usize>::new();
    let mut output_dirs = BTreeSet::<PathBuf>::new();
    let mut jobs = Vec::with_capacity(candidates.len());

    for path in candidates {
        let base_name = chart_base_name(&path)?;
        let unique_name = unique_chart_name(&base_name, &mut used_names);
        let out_chart_dir = Path::new(&args.out_dir).join(&unique_name);
        if !output_dirs.insert(out_chart_dir.clone()) {
            return Err(format!(
                "duplicate output directory computed for chart '{}': {}",
                unique_name,
                out_chart_dir.to_string_lossy()
            ));
        }
        jobs.push(BatchChartJob {
            name: unique_name,
            source_path: path.to_string_lossy().to_string(),
            out_chart_dir: out_chart_dir.to_string_lossy().to_string(),
        });
    }

    Ok(jobs)
}

fn unique_chart_name(base: &str, used: &mut BTreeMap<String, usize>) -> String {
    let count = used.entry(base.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 {
        return base.to_string();
    }
    format!("{base}-{}", *count)
}

fn chart_base_name(path: &Path) -> Result<String, String> {
    let raw = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid chart path '{}'", path.to_string_lossy()))?;
    let mut name = raw.to_string();
    if let Some(stripped) = name.strip_suffix(".tar.gz") {
        name = stripped.to_string();
    } else if let Some(stripped) = name.strip_suffix(".tgz") {
        name = stripped.to_string();
    }
    if name.trim().is_empty() {
        return Err(format!(
            "cannot derive chart name from path '{}'",
            path.to_string_lossy()
        ));
    }
    Ok(name)
}

fn is_chart_archive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.ends_with(".tgz") || name.ends_with(".tar.gz")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ImportSharedArgs;
    use tempfile::TempDir;

    #[test]
    fn discover_batch_jobs_finds_chart_dirs_and_archives() {
        let td = TempDir::new().expect("tmp");
        let charts = td.path().join("charts");
        fs::create_dir_all(charts.join("alpha")).expect("alpha");
        fs::create_dir_all(charts.join("nested")).expect("nested");
        fs::write(charts.join("alpha").join("Chart.yaml"), "name: alpha").expect("chart yaml");
        fs::write(charts.join("beta.tgz"), "archive").expect("archive");
        fs::write(charts.join("notes.txt"), "skip").expect("notes");

        let out = td.path().join("out");
        let args = BatchArgs {
            charts_dir: charts.to_string_lossy().to_string(),
            out_dir: out.to_string_lossy().to_string(),
            keep_going: false,
            import: ImportSharedArgs::default(),
        };

        let jobs = discover_batch_jobs(&args).expect("jobs");
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "alpha");
        assert_eq!(jobs[1].name, "beta");
    }

    #[test]
    fn discover_batch_jobs_deduplicates_output_names() {
        let td = TempDir::new().expect("tmp");
        let charts = td.path().join("charts");
        fs::create_dir_all(charts.join("dup")).expect("dup");
        fs::write(charts.join("dup").join("Chart.yaml"), "name: dup").expect("chart yaml");
        fs::write(charts.join("dup.tgz"), "archive").expect("archive");

        let args = BatchArgs {
            charts_dir: charts.to_string_lossy().to_string(),
            out_dir: td.path().join("out").to_string_lossy().to_string(),
            keep_going: false,
            import: ImportSharedArgs::default(),
        };

        let jobs = discover_batch_jobs(&args).expect("jobs");
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "dup");
        assert_eq!(jobs[1].name, "dup-2");
    }

    #[test]
    fn run_batch_convert_stops_on_first_error_without_keep_going() {
        let td = TempDir::new().expect("tmp");
        let charts = td.path().join("charts");
        fs::create_dir_all(charts.join("a")).expect("a");
        fs::create_dir_all(charts.join("b")).expect("b");
        fs::write(charts.join("a").join("Chart.yaml"), "name: a").expect("chart yaml");
        fs::write(charts.join("b").join("Chart.yaml"), "name: b").expect("chart yaml");

        let args = BatchArgs {
            charts_dir: charts.to_string_lossy().to_string(),
            out_dir: td.path().join("out").to_string_lossy().to_string(),
            keep_going: false,
            import: ImportSharedArgs::default(),
        };

        let mut called = Vec::new();
        let err = run_batch_convert(&args, |import| {
            called.push(import.path.clone());
            Err("boom".to_string())
        })
        .expect_err("must fail");
        assert_eq!(called.len(), 1);
        assert!(err.contains("failed=1"));
    }

    #[test]
    fn run_batch_convert_collects_errors_with_keep_going() {
        let td = TempDir::new().expect("tmp");
        let charts = td.path().join("charts");
        fs::create_dir_all(charts.join("a")).expect("a");
        fs::create_dir_all(charts.join("b")).expect("b");
        fs::write(charts.join("a").join("Chart.yaml"), "name: a").expect("chart yaml");
        fs::write(charts.join("b").join("Chart.yaml"), "name: b").expect("chart yaml");

        let mut args = BatchArgs {
            charts_dir: charts.to_string_lossy().to_string(),
            out_dir: td.path().join("out").to_string_lossy().to_string(),
            keep_going: true,
            import: ImportSharedArgs::default(),
        };
        args.import.verify_equivalence = true;

        let mut called = Vec::new();
        let err = run_batch_convert(&args, |import| {
            called.push((import.path.clone(), import.verify_equivalence));
            Err("failed".to_string())
        })
        .expect_err("must fail");

        assert_eq!(called.len(), 2);
        assert!(called.iter().all(|(_, verify)| *verify));
        assert!(err.contains("failed=2"));
    }
}
