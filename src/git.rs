use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

use git2::{Commit, DiffOptions, Error, ErrorCode, Repository, Signature};
use tempfile::NamedTempFile;

/// Gets a formatted diff from the git repository.
pub fn get_pretty_diff(repo: &Repository, context_lines: u32) -> Result<String, Error> {
    let mut output = String::new();
    let index = repo.index()?;
    let mut status_options = git2::StatusOptions::new();
    let statuses = repo.statuses(Some(&mut status_options))?;
    let has_staged = statuses.iter().any(|entry| {
        let status = entry.status();
        status.contains(git2::Status::INDEX_NEW)
            || status.contains(git2::Status::INDEX_MODIFIED)
            || status.contains(git2::Status::INDEX_DELETED)
            || status.contains(git2::Status::INDEX_RENAMED)
            || status.contains(git2::Status::INDEX_TYPECHANGE)
    });
    if !has_staged {
        log::warn!("No changes added");
        return Err(Error::from_str("No changes added"));
    }

    let head_tree = head_commit(repo)?.map(|commit| commit.tree()).transpose()?;

    let mut diff_options = DiffOptions::new();
    diff_options
        .context_lines(context_lines)
        .interhunk_lines(3)
        .id_abbrev(40)
        .ignore_whitespace(false)
        .patience(true)
        .minimal(true);

    let diff =
        repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut diff_options))?;
    let mut current_file: Option<PathBuf> = None;
    // Where the current delta's lines start in `output`, so a binary delta can be rolled back.
    let mut delta_start = 0;
    let mut binary = false;

    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        // 'F' opens every delta; a typechange is two deltas with the same path.
        if line.origin() == 'F' {
            binary = false;
            if let Some(path) = delta.new_file().path() {
                if current_file.as_deref() != Some(path) {
                    output.push_str(&format!("\npath: {}\n", path.display()));
                    current_file = Some(path.to_path_buf());
                }
            }
            delta_start = output.len();
            return true;
        }
        if binary {
            return true;
        }
        let is_binary = match line.origin() {
            '+' | '-' | ' ' => match std::str::from_utf8(line.content()) {
                Ok(text) => {
                    output.push(line.origin());
                    output.push(' ');
                    output.push_str(text);
                    false
                }
                // libgit2 only flags files with NUL bytes; other non-UTF-8 content is binary too.
                Err(_) => true,
            },
            'B' => true,
            _ => false,
        };
        if is_binary {
            binary = true;
            output.truncate(delta_start);
            output.push_str("(binary file, contents omitted)\n");
        }
        true
    })?;

    Ok(output)
}

/// Gets the default git signature from the repository config.
pub fn get_default_signature(
    repo: &Repository,
) -> Result<Signature<'_>, Box<dyn std::error::Error>> {
    let config = repo.config()?;
    let name = config.get_string("user.name")?;
    let email = config.get_string("user.email")?;
    Ok(Signature::now(&name, &email)?)
}

/// Gets a commit message interactively using an editor.
pub fn get_commit_message_interactively(initial_message: &str) -> io::Result<String> {
    let mut temp_file = NamedTempFile::new()?;
    writeln!(temp_file, "{initial_message}")?;
    temp_file.flush()?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    Command::new(editor).arg(temp_file.path()).status()?;

    let mut file = File::open(temp_file.path())?;
    let mut edited_message = String::new();
    io::Read::read_to_string(&mut file, &mut edited_message)?;

    Ok(edited_message)
}

/// Creates a commit with the given message.
pub fn create_commit(
    repo: &Repository,
    commit_message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut index = repo.index()?;
    index.write()?;

    let oid = index.write_tree()?;
    let tree = repo.find_tree(oid)?;
    let signature = get_default_signature(repo)?;
    let parent = head_commit(repo)?;
    let parents: Vec<&Commit<'_>> = parent.iter().collect();

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        commit_message,
        &tree,
        &parents,
    )?;

    Ok(())
}

/// Returns the HEAD commit, or `None` when HEAD points at a branch with no commits yet.
fn head_commit(repo: &Repository) -> Result<Option<Commit<'_>>, Error> {
    match repo.head() {
        Ok(head) => head.peel_to_commit().map(Some),
        Err(error) if error.code() == ErrorCode::UnbornBranch => Ok(None),
        Err(error) => Err(error),
    }
}

/// Returns up to ten recent non-merge commit messages for style examples.
pub fn get_commit_history(repo: &Repository) -> Result<Vec<String>, Error> {
    const MAX_COMMITS: usize = 10;
    const MAX_TRAVERSED_COMMITS: usize = 100;

    let Some(head) = head_commit(repo)? else {
        return Ok(Vec::new());
    };
    let mut revwalk = repo.revwalk()?;
    revwalk.push(head.id())?;
    revwalk.set_sorting(git2::Sort::TIME)?;

    let mut commits = Vec::with_capacity(MAX_COMMITS);
    for oid_result in revwalk.take(MAX_TRAVERSED_COMMITS) {
        let commit = repo.find_commit(oid_result?)?;
        if commit.parent_count() > 1 {
            continue;
        }
        if let Some(message) = commit.message() {
            commits.push(message.to_string());
        }
        if commits.len() == MAX_COMMITS {
            break;
        }
    }

    Ok(commits)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use git2::{Commit, Repository, Signature};
    use tempfile::tempdir;

    use super::*;

    fn signature() -> Signature<'static> {
        Signature::now("Test User", "test@example.com").unwrap()
    }

    fn commit_file(
        repo: &Repository,
        name: &str,
        content: &str,
        parents: &[&Commit<'_>],
    ) -> git2::Oid {
        let workdir = repo.workdir().unwrap();
        fs::write(workdir.join(name), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = signature();
        repo.commit(
            None,
            &signature,
            &signature,
            &format!("test: add {name}"),
            &tree,
            parents,
        )
        .unwrap()
    }

    #[test]
    fn unborn_repositories_have_empty_history() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();

        assert!(get_commit_history(&repo).unwrap().is_empty());
    }

    #[test]
    fn first_commit_uses_the_staged_diff_and_has_no_parents() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_str("user.name", "Test User")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("user.email", "test@example.com")
            .unwrap();
        fs::write(temp.path().join("first.txt"), "first commit\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("first.txt")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("untracked.txt"), "untracked\n").unwrap();

        assert!(get_pretty_diff(&repo, 3).unwrap().contains("first commit"));
        create_commit(&repo, "feat: initial commit").unwrap();

        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 0);
        assert_eq!(
            get_commit_history(&repo).unwrap(),
            vec!["feat: initial commit"]
        );
    }

    #[test]
    fn history_skips_merge_commits() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let first = commit_file(&repo, "first.txt", "first", &[]);
        let first_commit = repo.find_commit(first).unwrap();
        let second = commit_file(&repo, "second.txt", "second", &[&first_commit]);
        let second_commit = repo.find_commit(second).unwrap();
        let side = commit_file(&repo, "side.txt", "side", &[&first_commit]);
        let side_commit = repo.find_commit(side).unwrap();

        let merge = commit_file(&repo, "merge.txt", "merge", &[&second_commit, &side_commit]);
        repo.reference("refs/heads/master", merge, true, "test merge")
            .unwrap();
        repo.set_head("refs/heads/master").unwrap();

        let history = get_commit_history(&repo).unwrap();
        assert_eq!(history.len(), 3);
        assert!(history.iter().all(|message| !message.contains("merge")));
    }

    #[test]
    fn binary_files_are_listed_without_contents() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let files: [(&str, &[u8]); 3] = [
            ("text.txt", b"hello\n"),
            ("nul.dat", b"PNG\0\0data\n"),
            ("latin1.txt", b"caf\xe9\xff\xfe\n"),
        ];
        let mut index = repo.index().unwrap();
        for (name, content) in files {
            fs::write(temp.path().join(name), content).unwrap();
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();

        let diff = get_pretty_diff(&repo, 3).unwrap();
        assert_eq!(
            diff,
            "\npath: latin1.txt\n(binary file, contents omitted)\n\
             \npath: nul.dat\n(binary file, contents omitted)\n\
             \npath: text.txt\n+ hello\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn binary_flag_does_not_leak_into_typechange_of_same_path() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        let oid = commit_file(&repo, "x", "\0", &[]);
        repo.set_head_detached(oid).unwrap();
        fs::remove_file(temp.path().join("x")).unwrap();
        std::os::unix::fs::symlink("target", temp.path().join("x")).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("x")).unwrap();
        index.write().unwrap();

        assert_eq!(
            get_pretty_diff(&repo, 3).unwrap(),
            "\npath: x\n(binary file, contents omitted)\n+ target"
        );
    }
}
