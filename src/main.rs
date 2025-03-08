use git2::{DiffOptions, Error, Repository};

fn print_pretty_diff(repo: &Repository, context_lines: u32) -> Result<(), Error> {
    // Retrieve the index from the repository
    let index = repo.index()?;
    // Retrieve the HEAD commit
    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    // Retrieve the tree associated with the HEAD commit
    let head_tree = head_commit.tree()?;

    // Create diff options
    let mut diff_options = DiffOptions::new();
    diff_options
        .context_lines(context_lines) // Set the number of context lines
        .interhunk_lines(3) // Set the number of inter-hunk lines
        .id_abbrev(40) // Set the abbreviation length for object IDs
        .ignore_whitespace(false) // Do not ignore whitespace changes
        .patience(true) // Use the "patience diff" algorithm
        .minimal(true); // Spend extra time to find minimal diff

    // Generate a diff between the HEAD tree and the index
    let diff = repo.diff_tree_to_index(Some(&head_tree), Some(&index), Some(&mut diff_options))?;

    // Track the current file path as an Option<String> to simplify comparisons
    let mut current_file: Option<String> = None;

    // Iterate over each diff delta (i.e., each file change)
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let line_content = String::from_utf8_lossy(line.content());

        // Check if this is a new file by comparing path strings
        if let Some(path) = delta.new_file().path() {
            let path_str = path.to_string_lossy().to_string();

            // If this is a different file than what we've been processing
            if current_file.as_ref() != Some(&path_str) {
                current_file = Some(path_str);
                println!("\npath: {}", path.display());
            }
        }

        // Print lines based on their origin, preserving the diff symbols
        match line.origin() {
            '+' | '-' | ' ' => {
                // Print the line with its origin character (+ for addition, - for deletion, space for context)
                print!("{} {}", line.origin(), line_content);
            }
            _ => {} // Skip other lines (file headers, etc.)
        }

        true
    })?;

    Ok(())
}

fn main() -> Result<(), Error> {
    let repo = Repository::open("./")?;

    print_pretty_diff(&repo, 3)
}
