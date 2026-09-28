use std::fs;
use std::path::{Path, PathBuf};

const MAX_GIT_WRITABLE_ROOTS: usize = 2;

/// Git metadata dirs this worktree needs, when they live outside the workspace.
///
/// A submodule checkout has a `.git` file that points at
/// `$super/.git/modules/...`. Bubblewrap binds only the workspace read-write, so
/// `git pull` cannot write `FETCH_HEAD` unless that gitdir is also writable.
///
/// The path must contain `/.git` so a crafted `gitdir:` line cannot bind
/// `$HOME`, `/etc`, or another non-git tree.
pub(crate) fn workspace_git_writable_roots(workspace: &Path) -> Vec<PathBuf> {
    let Ok(workspace) = workspace.canonicalize() else {
        return Vec::new();
    };
    let Some(git_dir) = read_git_dir(&workspace) else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    push_external_git_root(&mut roots, &workspace, git_dir);
    if let Some(common) = roots
        .first()
        .cloned()
        .and_then(|git_dir| read_common_dir(&git_dir))
    {
        push_external_git_root(&mut roots, &workspace, common);
    }
    roots
}

fn push_external_git_root(roots: &mut Vec<PathBuf>, workspace: &Path, candidate: PathBuf) {
    if roots.len() >= MAX_GIT_WRITABLE_ROOTS {
        return;
    }
    if is_inside(workspace, &candidate) {
        return;
    }
    if !looks_like_git_metadata(&candidate) {
        return;
    }
    if !candidate.is_dir() {
        return;
    }
    if roots.iter().any(|root| root == &candidate) {
        return;
    }
    roots.push(candidate);
}

fn read_git_dir(workspace: &Path) -> Option<PathBuf> {
    let git_link = workspace.join(".git");
    let meta = fs::symlink_metadata(&git_link).ok()?;
    if meta.is_dir() {
        return git_link.canonicalize().ok();
    }
    if fs::metadata(&git_link).ok()?.is_dir() {
        return git_link.canonicalize().ok();
    }
    let contents = fs::read_to_string(&git_link).ok()?;
    let raw = parse_gitdir_line(&contents)?;
    resolve_against(workspace, raw)
}

fn read_common_dir(git_dir: &Path) -> Option<PathBuf> {
    let contents = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let raw = contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    resolve_against(git_dir, raw)
}

fn parse_gitdir_line(contents: &str) -> Option<&str> {
    contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix("gitdir:")
            .map(str::trim)
            .filter(|path| !path.is_empty())
    })
}

fn resolve_against(base: &Path, raw: &str) -> Option<PathBuf> {
    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    resolved.canonicalize().ok()
}

fn looks_like_git_metadata(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    value.ends_with("/.git") || value.contains("/.git/")
}

fn is_inside(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    use tempfile::TempDir;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        let mut file = fs::File::create(path).expect("create");
        file.write_all(contents.as_bytes()).expect("write");
    }

    #[test]
    fn ordinary_repo_needs_no_extra_roots() {
        let temp = TempDir::new().expect("temp");
        fs::create_dir(temp.path().join(".git")).expect("git dir");
        assert!(workspace_git_writable_roots(temp.path()).is_empty());
    }

    #[test]
    fn submodule_gitdir_outside_the_worktree_is_writable() {
        let temp = TempDir::new().expect("temp");
        let super_git = temp
            .path()
            .join("paid_projects/.git/modules/numero/modules/spiderman");
        let worktree = temp.path().join("paid_projects/numero/spiderman");
        fs::create_dir_all(&super_git).expect("gitdir");
        fs::create_dir_all(&worktree).expect("worktree");
        write_file(
            &worktree.join(".git"),
            &format!("gitdir: {}\n", super_git.display()),
        );

        let roots = workspace_git_writable_roots(&worktree);
        assert_eq!(roots, vec![super_git.canonicalize().expect("gitdir")]);
    }

    #[test]
    fn relative_gitdir_and_commondir_are_resolved() {
        let temp = TempDir::new().expect("temp");
        let common = temp.path().join("repo/.git");
        let git_dir = temp.path().join("repo/.git/worktrees/feature");
        let worktree = temp.path().join("feature");
        fs::create_dir_all(&git_dir).expect("gitdir");
        fs::create_dir_all(&common).expect("common");
        fs::create_dir_all(&worktree).expect("worktree");
        write_file(
            &worktree.join(".git"),
            "gitdir: ../repo/.git/worktrees/feature\n",
        );
        write_file(&git_dir.join("commondir"), "../..\n");

        let mut roots = workspace_git_writable_roots(&worktree);
        roots.sort();
        let mut expected = vec![
            git_dir.canonicalize().expect("gitdir"),
            common.canonicalize().expect("common"),
        ];
        expected.sort();
        assert_eq!(roots, expected);
    }

    #[test]
    fn gitdir_outside_git_metadata_is_ignored() {
        let temp = TempDir::new().expect("temp");
        let secret = temp.path().join("home/nick/.ssh");
        let worktree = temp.path().join("workspace");
        fs::create_dir_all(&secret).expect("secret");
        fs::create_dir_all(&worktree).expect("worktree");
        write_file(
            &worktree.join(".git"),
            &format!("gitdir: {}\n", secret.display()),
        );
        assert!(workspace_git_writable_roots(&worktree).is_empty());
    }

    #[test]
    fn parse_gitdir_line_skips_noise() {
        assert_eq!(
            parse_gitdir_line("# comment\ngitdir: /tmp/example/.git/modules/app\n"),
            Some("/tmp/example/.git/modules/app")
        );
        assert_eq!(parse_gitdir_line("not a git file\n"), None);
    }
}
