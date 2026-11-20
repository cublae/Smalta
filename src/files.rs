//! Files under the home directory, for the launcher's `/`.
//!
//! The home directory is walked once when the first `/` is typed and kept for
//! a minute. The walk stays out of hidden directories and the usual heaps of
//! generated files, and stops at a depth and a count that keep it to a
//! fraction of a second on an ordinary home.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Directories never walked into, besides hidden ones.
const SKIP: [&str; 9] = ["node_modules", "target", "result", "__pycache__", "venv", "build", "dist", "snap", "go"];
const DEPTH: usize = 6;
const LIMIT: usize = 40_000;
const FRESH: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct File {
    pub name: String,
    /// From the home directory, `Documents/notes.txt`.
    pub relative: String,
    pub path: PathBuf,
    pub dir: bool,
}

#[derive(Default)]
pub struct Index {
    pub files: Vec<File>,
    built: Option<Instant>,
}

impl Index {
    /// The files, walked again if the last walk is over a minute old.
    pub fn files(&mut self) -> &[File] {
        if self.built.is_none_or(|t| t.elapsed() > FRESH) {
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
            self.files.clear();
            walk(&home, &mut self.files);
            self.built = Some(Instant::now());
        }
        &self.files
    }
}

/// Breadth first: what is near the top is found even when the limit cuts the
/// walk short.
fn walk(home: &Path, out: &mut Vec<File>) {
    let mut queue = std::collections::VecDeque::from([(home.to_path_buf(), 0)]);
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if out.len() >= LIMIT {
                return;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            // Not following links: a link back up would walk forever.
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            let relative = path.strip_prefix(home).unwrap_or(&path).to_string_lossy().into_owned();
            let dir = kind.is_dir();
            if dir && depth + 1 < DEPTH && !SKIP.contains(&name.as_str()) {
                queue.push_back((path.clone(), depth + 1));
            }
            out.push(File { name, relative, path, dir });
        }
    }
}
