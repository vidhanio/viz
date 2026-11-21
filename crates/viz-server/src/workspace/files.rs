use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::{
    fs,
    io::{AsyncBufReadExt as _, BufReader},
    process::Command,
    sync::{Mutex, OnceCell},
};

const WALK_LIMIT: usize = 20_000;
const TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub(super) struct Files(Mutex<HashMap<String, Cached>>);

#[derive(Debug)]
struct Cached {
    since: Instant,
    files: Arc<OnceCell<Vec<String>>>,
}

impl Files {
    pub(super) async fn search(&self, cwd: &str, query: &str, limit: usize) -> Vec<String> {
        let files = {
            let mut cache = self.0.lock().await;
            cache.retain(|_, cached| cached.since.elapsed() < TTL);
            let known = cache.get(cwd).map(|cached| Arc::clone(&cached.files));
            let files = known.unwrap_or_else(|| {
                let files = Arc::new(OnceCell::new());
                cache.insert(
                    cwd.to_owned(),
                    Cached {
                        since: Instant::now(),
                        files: Arc::clone(&files),
                    },
                );
                files
            });
            drop(cache);
            files
        };
        let files = files.get_or_init(|| list(Path::new(cwd))).await;
        let query: Vec<_> = query.trim().chars().flat_map(char::to_lowercase).collect();
        if query.is_empty() {
            return files.iter().take(limit).cloned().collect();
        }
        let mut names = Vec::new();
        let mut candidates = Vec::new();
        for file in files {
            let basename = file.rsplit('/').next().unwrap_or(file);
            if fuzzy(&query, basename) {
                names.push(file.clone());
                if names.len() == limit {
                    return names;
                }
            } else if candidates.len() < limit && fuzzy(&query, file) {
                candidates.push(file);
            }
        }
        names.extend(candidates.into_iter().take(limit - names.len()).cloned());
        names
    }
}

fn fuzzy(query: &[char], candidate: &str) -> bool {
    let mut candidate = candidate.chars().flat_map(char::to_lowercase);
    query
        .iter()
        .all(|&query_char| candidate.any(|ch| ch == query_char))
}

async fn list(cwd: &Path) -> Vec<String> {
    let mut paths = if let Some(files) = git_files(cwd).await {
        files
    } else {
        walk(cwd).await
    };
    paths
        .sort_unstable_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
    paths
}

async fn git_files(cwd: &Path) -> Option<Vec<String>> {
    let mut git = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = BufReader::new(git.stdout.take()?).split(0);
    let mut paths = HashSet::new();
    while let Some(bytes) = stdout.next_segment().await.ok()? {
        if let Ok(path) = String::from_utf8(bytes) {
            paths.insert(path);
        }
        if paths.len() == WALK_LIMIT {
            return Some(paths.into_iter().collect());
        }
    }
    git.wait()
        .await
        .ok()?
        .success()
        .then(|| paths.into_iter().collect())
}

async fn walk(cwd: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut dirs = vec![PathBuf::new()];
    let mut visited = 0;
    while let Some(dir) = dirs.pop() {
        let Ok(mut entries) = fs::read_dir(cwd.join(&dir)).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            visited += 1;
            if visited > WALK_LIMIT {
                return paths;
            }
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let path = dir.join(name);
            if file_type.is_dir() {
                if !name.starts_with('.') && !matches!(name, "target" | "node_modules") {
                    dirs.push(path);
                }
            } else if (file_type.is_file() || file_type.is_symlink())
                && let Some(path) = path.into_os_string().into_string().ok()
            {
                paths.push(path);
            }
        }
    }
    paths
}
