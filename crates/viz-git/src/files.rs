//! Ignore-aware path lookup using the same rules inside and outside Git.

use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;
use nucleo_matcher::{
    Config, Matcher,
    pattern::{CaseMatching, Normalization, Pattern},
};
use tokio_util::sync::CancellationToken;

use crate::{Error, blocking, check};

/// Find matching files relative to the root, sorted by score then path.
///
/// # Errors
/// Returns an error if walking the root fails or the request is cancelled.
pub async fn find_files(
    root: &Path,
    query: &str,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>, Error> {
    let (root, query) = (root.to_owned(), query.to_owned());
    blocking(cancel, move |cancel| {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut utf32 = Vec::new();
        let mut selected = BinaryHeap::new();
        let mut walk = WalkBuilder::new(&root);
        walk.require_git(false)
            .filter_entry(|entry| entry.file_name() != ".git");
        for entry in walk.build() {
            check(cancel)?;
            let entry = entry?;
            if entry.file_type().is_none_or(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry
                .path()
                .strip_prefix(&root)
                .map_err(|_| Error::Invalid("walk path"))?;
            let path_string = path.to_string_lossy();
            if let Some(score) = pattern.score(
                nucleo_matcher::Utf32Str::new(&path_string, &mut utf32),
                &mut matcher,
            ) {
                selected.push((Reverse(score), path.to_owned()));
                if selected.len() > limit {
                    selected.pop();
                }
            }
        }
        Ok(selected
            .into_sorted_vec()
            .into_iter()
            .map(|(_, path)| path)
            .collect())
    })
    .await
}
