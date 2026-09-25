use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobBuilder, GlobMatcher};
use serde::Serialize;

use crate::discovery::{self, Document};
use crate::registry::modified_at;

/// More would stop being a list of favourites, or of one session's notes, and
/// make the picker slow.
pub const MAX_DOCUMENTS: usize = 200;
/// Directory entries one refresh may visit, so `~/**/*.md` cannot stall it.
const SCAN_BUDGET: usize = 20_000;
const MAX_DEPTH: usize = 16;

#[derive(Debug, Default, Serialize)]
pub struct Listing {
    pub documents: Vec<Document>,
    /// Matching files found, which exceeds `documents.len()` when capped.
    pub found: usize,
    pub warnings: Vec<String>,
}

/// Expands configured bookmark entries into Markdown files. Entries starting
/// with `/` or `~` are absolute; any other entry is relative to `cwd`, the
/// project of the viewer's session, and is skipped without one. An entry is a
/// file, a directory (its Markdown, recursively) or a glob. Missing files are
/// left out quietly, since a relative entry may exist only in some projects.
pub fn list(entries: &[String], cwd: Option<&Path>, home: &Path) -> Listing {
    let mut scan = Scan { budget: SCAN_BUDGET, seen: HashSet::new(), listing: Listing::default() };
    for entry in entries {
        let Some((path, base)) = resolve(entry, cwd, home) else { continue };
        let mut found = Vec::new();
        let result = if has_glob(entry) {
            glob_files(&path, &mut scan.budget, &mut found)
        } else if path.is_dir() {
            walk(&path, &mut scan.budget, &mut |file| found.push(file.to_owned()));
            Ok(())
        } else if path.is_file() && !discovery::is_markdown(&path) {
            Err("not a Markdown file".to_string())
        } else if path.is_file() {
            found.push(path);
            Ok(())
        } else {
            Ok(())
        };
        if let Err(message) = result {
            scan.listing.warnings.push(format!("{entry}: {message}"));
        }
        found.sort();
        for file in found {
            let touched_at = modified_at(&file).unwrap_or(0);
            scan.add(file, touched_at, &base, home);
        }
        if scan.budget == 0 {
            scan.listing.warnings.push(format!("{entry}: stopped after scanning {SCAN_BUDGET} entries"));
            break;
        }
    }
    scan.listing
}

/// Markdown under one folder, such as a session's scratchpad, newest first and
/// labelled relative to it, with the same limits as bookmarks.
pub fn folder(dir: &Path) -> Listing {
    let mut scan = Scan { budget: SCAN_BUDGET, seen: HashSet::new(), listing: Listing::default() };
    let mut found = Vec::new();
    walk(dir, &mut scan.budget, &mut |file| found.push((modified_at(file).unwrap_or(0), file.to_owned())));
    found.sort_by(|a, b| b.cmp(a));
    for (touched_at, file) in found {
        scan.add(file, touched_at, &Base::Project(dir.to_owned()), dir);
    }
    if scan.budget == 0 {
        scan.listing.warnings.push(format!("stopped after scanning {SCAN_BUDGET} entries"));
    }
    scan.listing
}

struct Scan {
    budget: usize,
    seen: HashSet<PathBuf>,
    listing: Listing,
}

impl Scan {
    fn add(&mut self, path: PathBuf, touched_at: i64, base: &Base, home: &Path) {
        if !self.seen.insert(path.clone()) {
            return;
        }
        self.listing.found += 1;
        if self.listing.documents.len() == MAX_DOCUMENTS {
            return;
        }
        let label = match base {
            Base::Project(cwd) => path.strip_prefix(cwd).unwrap_or(&path).display().to_string(),
            Base::Absolute => match path.strip_prefix(home) {
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => path.display().to_string(),
            },
        };
        self.listing.documents.push(Document { path, touched_at, label, scanned: false, shared: false });
    }
}

enum Base {
    Absolute,
    Project(PathBuf),
}

fn resolve(entry: &str, cwd: Option<&Path>, home: &Path) -> Option<(PathBuf, Base)> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if entry == "~" {
        return Some((home.to_owned(), Base::Absolute));
    }
    if let Some(rest) = entry.strip_prefix("~/") {
        return Some((home.join(rest), Base::Absolute));
    }
    if Path::new(entry).is_absolute() {
        return Some((PathBuf::from(entry), Base::Absolute));
    }
    let cwd = cwd?;
    // Collecting components drops `.` segments, so `./docs` lists the same
    // paths as `docs`.
    Some((cwd.join(entry).components().collect(), Base::Project(cwd.to_owned())))
}

fn has_glob(entry: &str) -> bool {
    entry.contains(['*', '?', '[', '{'])
}

/// Walks from the deepest directory before the first wildcard, matching whole
/// paths, so `docs/*.md` never scans more than `docs`.
fn glob_files(pattern: &Path, budget: &mut usize, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let text = pattern.to_str().ok_or("path is not valid UTF-8")?;
    let matcher: GlobMatcher = GlobBuilder::new(text)
        .literal_separator(true)
        .build()
        .map(|glob: Glob| glob.compile_matcher())
        .map_err(|e| e.kind().to_string())?;
    let mut root = PathBuf::new();
    for component in pattern.components() {
        if matches!(component, Component::Normal(part) if has_glob(&part.to_string_lossy())) {
            break;
        }
        root.push(component);
    }
    if root.is_dir() {
        walk(&root, budget, &mut |file| {
            if matcher.is_match(file) {
                found.push(file.to_owned());
            }
        });
    }
    Ok(())
}

/// Markdown files under `dir`. Hidden entries are skipped and directory
/// symlinks are not followed, which rules out loops; file symlinks are, since
/// agent instruction files are often links into a dotfiles repository.
fn walk(dir: &Path, budget: &mut usize, visit: &mut dyn FnMut(&Path)) {
    let mut pending = vec![(dir.to_owned(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if discovery::is_markdown(&path) && (kind.is_file() || path.is_file()) {
                visit(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "peekback-bookmarks-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "# note\n").unwrap();
            path
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn labels(bookmarks: &Listing) -> Vec<&str> {
        bookmarks.documents.iter().map(|d| d.label.as_str()).collect()
    }

    fn entries(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absolute_home_and_relative_entries_resolve_and_label_by_origin() {
        let temp = Temp::new();
        let home = temp.0.join("home");
        let project = temp.0.join("project");
        temp.file("home/.claude/CLAUDE.md");
        temp.file("project/CLAUDE.md");
        let other = temp.file("elsewhere/notes.md");
        let list_in = |cwd| {
            list(&entries(&["~/.claude/CLAUDE.md", other.to_str().unwrap(), "CLAUDE.md", "missing.md"]), cwd, &home)
        };
        let bookmarks = list_in(Some(&project));
        assert_eq!(labels(&bookmarks), ["~/.claude/CLAUDE.md", other.to_str().unwrap(), "CLAUDE.md"]);
        assert!(bookmarks.warnings.is_empty());
        // Without a session, relative entries have nothing to resolve against.
        assert_eq!(labels(&list_in(None)), ["~/.claude/CLAUDE.md", other.to_str().unwrap()]);
    }

    #[test]
    fn directories_and_globs_find_markdown_skipping_hidden_and_other_files() {
        let temp = Temp::new();
        temp.file("project/docs/b.md");
        temp.file("project/docs/a.markdown");
        temp.file("project/docs/deep/c.md");
        temp.file("project/docs/.drafts/hidden.md");
        temp.file("project/docs/code.rs");
        let project = temp.0.join("project");
        let listed = |entry: &str| list(&entries(&[entry]), Some(&project), &temp.0);
        assert_eq!(labels(&listed("docs")), ["docs/a.markdown", "docs/b.md", "docs/deep/c.md"]);
        assert_eq!(labels(&listed("docs/*.md")), ["docs/b.md"]);
        assert_eq!(labels(&listed("docs/**/*.md")), ["docs/b.md", "docs/deep/c.md"]);
        assert_eq!(labels(&listed("docs/{a,b}.*")), ["docs/a.markdown", "docs/b.md"]);
    }

    #[test]
    fn entries_keep_config_order_without_duplicates() {
        let temp = Temp::new();
        temp.file("p/z.md");
        temp.file("p/a.md");
        let project = temp.0.join("p");
        let bookmarks = list(&entries(&["z.md", ".", "./a.md"]), Some(&project), &temp.0);
        assert_eq!(labels(&bookmarks), ["z.md", "a.md"]);
        assert_eq!(bookmarks.documents[1].path, project.join("a.md"));
        assert_eq!(bookmarks.found, 2);
    }

    #[test]
    fn file_symlinks_are_followed_but_directory_symlinks_are_not() {
        let temp = Temp::new();
        let target = temp.file("dotfiles/CLAUDE.md");
        fs::create_dir_all(temp.0.join("p")).unwrap();
        symlink(&target, temp.0.join("p/CLAUDE.md")).unwrap();
        symlink(&temp.0, temp.0.join("p/loop")).unwrap();
        let bookmarks = list(&entries(&["."]), Some(&temp.0.join("p")), &temp.0);
        assert_eq!(labels(&bookmarks), ["CLAUDE.md"]);
    }

    #[test]
    fn bad_entries_warn_and_large_results_are_capped() {
        let temp = Temp::new();
        temp.file("p/code.rs");
        for index in 0..MAX_DOCUMENTS + 5 {
            temp.file(&format!("p/many/{index:03}.md"));
        }
        let project = temp.0.join("p");
        let bookmarks = list(&entries(&["code.rs", "docs/[.md", "many"]), Some(&project), &temp.0);
        assert_eq!(bookmarks.warnings.len(), 2, "{:?}", bookmarks.warnings);
        assert!(bookmarks.warnings[0].starts_with("code.rs: not a Markdown file"));
        assert!(bookmarks.warnings[1].starts_with("docs/[.md: "));
        assert_eq!(bookmarks.documents.len(), MAX_DOCUMENTS);
        assert_eq!(bookmarks.found, MAX_DOCUMENTS + 5);
    }

    #[test]
    fn a_folder_lists_its_markdown_newest_first_relative_to_it() {
        let temp = Temp::new();
        let old = temp.file("plans/old.md");
        let new = temp.file("new.md");
        temp.file(".hidden/secret.md");
        fs::write(temp.0.join("data.json"), "{}").unwrap();
        let at = |secs| std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        fs::File::options().write(true).open(&old).unwrap().set_modified(at(1_000)).unwrap();
        fs::File::options().write(true).open(&new).unwrap().set_modified(at(2_000)).unwrap();
        let listing = folder(&temp.0);
        assert_eq!(labels(&listing), ["new.md", "plans/old.md"]);
        assert_eq!(listing.documents[1].touched_at, 1_000);
        assert!(folder(&temp.0.join("missing")).documents.is_empty());
    }
}
