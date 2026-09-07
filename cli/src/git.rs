use std::{
    collections::BTreeMap,
    env, fs,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{CompileError, DependencySpec, GitSelector};

pub const LOCKFILE: &str = "Ruddy.lock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub version: u32,
    #[serde(default, rename = "git", skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<LockedGit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedGit {
    pub url: String,
    #[serde(flatten)]
    pub selector: LockedSelector,
    pub commit: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedSelector {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

pub(crate) struct Resolver {
    home: Option<PathBuf>,
    lock_path: PathBuf,
    locked: BTreeMap<(String, LockedSelectorKey), String>,
    resolved: BTreeMap<(String, LockedSelectorKey), String>,
    had_lockfile: bool,
    // The global advisory lock protects cache installation and restored
    // worktrees until compilation has finished reading them. The OS releases
    // it if the process exits, including after a crash.
    cache_lock: Option<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LockedSelectorKey(String, String);

impl LockedSelectorKey {
    fn new(selector: GitSelector<'_>) -> Self {
        match selector {
            GitSelector::Default => Self("default".into(), String::new()),
            GitSelector::Branch(value) => Self("branch".into(), value.into()),
            GitSelector::Tag(value) => Self("tag".into(), value.into()),
            GitSelector::Rev(value) => Self("rev".into(), value.into()),
        }
    }
}

impl Resolver {
    pub(crate) fn new(root: &Path) -> Result<Self, CompileError> {
        let lock_path = root.join(LOCKFILE);
        let lock = match fs::read_to_string(&lock_path) {
            Ok(source) => Some(toml::from_str::<Lockfile>(&source).map_err(|error| {
                CompileError::report(
                    "lockfile-invalid",
                    format!("`{}` contains invalid lock settings", lock_path.display()),
                )
                .with_note(crate::toml_error_note(&source, &error))
                .with_help("remove `Ruddy.lock` to regenerate it, or fix the named field")
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(CompileError::report(
                    "lockfile-unreadable",
                    format!("could not read `{}`", lock_path.display()),
                )
                .with_note(error.to_string()));
            }
        };
        if let Some(lock) = &lock
            && lock.version != 1
        {
            return Err(CompileError::report(
                "lockfile-version-unsupported",
                format!(
                    "`{}` uses lock format version {}, but this Ruddy supports version 1",
                    lock_path.display(),
                    lock.version
                ),
            )
            .with_help("update Ruddy or remove `Ruddy.lock` to regenerate it"));
        }
        let had_lockfile = lock.is_some();
        let mut locked = BTreeMap::new();
        for entry in lock.into_iter().flat_map(|lock| lock.entries) {
            if !entry.url.starts_with("https://") {
                return Err(CompileError::report(
                    "lockfile-insecure-git-url",
                    "a Git URL in `Ruddy.lock` must use HTTPS",
                )
                .with_note(format!("in `{}`", lock_path.display()))
                .with_help("remove `Ruddy.lock` to regenerate it from `Ruddy.toml`"));
            }
            validate_full_commit(&entry.commit).map_err(|message| {
                CompileError::report(
                    "lockfile-invalid-commit",
                    "a Git commit in `Ruddy.lock` is invalid",
                )
                .with_note(message)
                .with_note(format!("in `{}`", lock_path.display()))
            })?;
            let selector = selector_from_locked(&entry.selector)
                .map_err(|error| error.with_note(format!("in `{}`", lock_path.display())))?;
            validate_selector(selector).map_err(|message| {
                CompileError::report(
                    "lockfile-invalid-selector",
                    "a Git branch, tag, or revision in `Ruddy.lock` is invalid",
                )
                .with_note(message)
                .with_note(format!("in `{}`", lock_path.display()))
            })?;
            if let GitSelector::Rev(revision) = selector
                && !entry
                    .commit
                    .to_ascii_lowercase()
                    .starts_with(&revision.to_ascii_lowercase())
            {
                return Err(CompileError::report(
                    "lockfile-revision-mismatch",
                    format!(
                        "revision `{revision}` does not match locked commit `{}`",
                        entry.commit
                    ),
                )
                .with_note(format!("in `{}`", lock_path.display()))
                .with_help("remove `Ruddy.lock` to resolve and lock the revision again"));
            }
            let key = (entry.url, LockedSelectorKey::new(selector));
            if locked
                .insert(key, entry.commit.to_ascii_lowercase())
                .is_some()
            {
                return Err(CompileError::report(
                    "lockfile-duplicate-git-source",
                    "`Ruddy.lock` contains the same Git source more than once",
                )
                .with_note(format!("in `{}`", lock_path.display()))
                .with_help("remove `Ruddy.lock` to regenerate it"));
            }
        }
        Ok(Self {
            home: None,
            lock_path,
            locked,
            resolved: BTreeMap::new(),
            had_lockfile,
            cache_lock: None,
        })
    }

    pub(crate) fn resolve(&mut self, spec: &DependencySpec) -> Result<PathBuf, CompileError> {
        let url = spec.git().expect("called for a Git dependency");
        let selector = spec.selector()?;
        validate_selector(selector).map_err(|message| {
            CompileError::report("dependency-selector-invalid", message)
                .with_help("choose a valid Git branch, tag, or 7–40 character hexadecimal revision")
        })?;
        if self.home.is_none() {
            self.home = Some(ruddy_home()?);
        }
        let home = self.home.as_deref().expect("initialized above");
        let key = (url.to_string(), LockedSelectorKey::new(selector));
        if let Some(commit) = self.resolved.get(&key) {
            return Ok(checkout_path(home, url, selector, commit));
        }
        if self.cache_lock.is_none() {
            self.cache_lock = Some(acquire_cache_lock(home)?);
        }
        clean_stale_temporary_checkouts(home, url, selector)?;
        let locked = self.locked.get(&key).cloned();
        let (checkout, commit) = if let Some(commit) = locked {
            let checkout = checkout_path(home, url, selector, &commit);
            if checkout.exists() && restore_checkout(&checkout, &commit).is_ok() {
                (checkout, commit)
            } else {
                if checkout.exists() {
                    fs::remove_dir_all(&checkout).map_err(|error| {
                        cache_error(
                            format!(
                                "could not replace cached Git dependency `{}`",
                                checkout.display()
                            ),
                            error,
                        )
                    })?;
                }
                (clone_locked(home, url, selector, &commit)?, commit)
            }
        } else {
            clone_unlocked(home, url, selector)?
        };
        self.resolved.insert(key, commit);
        Ok(checkout)
    }

    pub(crate) fn write_if_changed(&self) -> Result<(), CompileError> {
        if self.resolved.is_empty() && !self.had_lockfile {
            return Ok(());
        }
        let entries = self
            .resolved
            .iter()
            .map(|((url, key), commit)| LockedGit {
                url: url.clone(),
                selector: selector_to_locked(key),
                commit: commit.clone(),
            })
            .collect();
        let source = toml::to_string_pretty(&Lockfile {
            version: 1,
            entries,
        })
        .map_err(|error| {
            CompileError::report("lockfile-write-failed", "could not prepare `Ruddy.lock`")
                .with_note(error.to_string())
        })?;
        if fs::read_to_string(&self.lock_path).ok().as_deref() == Some(&source) {
            return Ok(());
        }
        crate::replace_file(&self.lock_path, source.as_bytes()).map_err(|error| {
            CompileError::report(
                "lockfile-write-failed",
                format!("could not write `{}`", self.lock_path.display()),
            )
            .with_note(error.to_string())
        })
    }
}

fn selector_from_locked(selector: &LockedSelector) -> Result<GitSelector<'_>, CompileError> {
    let values = [
        selector.branch.as_deref(),
        selector.tag.as_deref(),
        selector.rev.as_deref(),
    ];
    if values.iter().flatten().count() > 1 {
        return Err(CompileError::report(
            "lockfile-selectors-conflict",
            "a Git entry in `Ruddy.lock` selects more than one branch, tag, or revision",
        )
        .with_help("remove `Ruddy.lock` to regenerate it"));
    }
    Ok(if let Some(v) = values[0] {
        GitSelector::Branch(v)
    } else if let Some(v) = values[1] {
        GitSelector::Tag(v)
    } else if let Some(v) = values[2] {
        GitSelector::Rev(v)
    } else {
        GitSelector::Default
    })
}

fn selector_to_locked(key: &LockedSelectorKey) -> LockedSelector {
    match key.0.as_str() {
        "branch" => LockedSelector {
            branch: Some(key.1.clone()),
            ..Default::default()
        },
        "tag" => LockedSelector {
            tag: Some(key.1.clone()),
            ..Default::default()
        },
        "rev" => LockedSelector {
            rev: Some(key.1.clone()),
            ..Default::default()
        },
        _ => LockedSelector::default(),
    }
}

pub(crate) fn canonical_checkouts_root() -> Option<PathBuf> {
    ruddy_home()
        .ok()
        .and_then(|home| fs::canonicalize(git_cache(&home).join("checkouts")).ok())
        .filter(|path| path.is_dir())
}

pub fn ruddy_home() -> Result<PathBuf, CompileError> {
    let configured = if let Some(path) = env::var_os("RUDDY_HOME").filter(|value| !value.is_empty())
    {
        PathBuf::from(path)
    } else {
        env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|home| PathBuf::from(home).join(".ruddy"))
            .ok_or_else(|| {
                CompileError::report(
                    "ruddy-home-unavailable",
                    "Ruddy does not know where to store downloaded dependencies",
                )
                .with_help("set `RUDDY_HOME` to a writable folder")
            })?
    };
    if configured.is_absolute() {
        Ok(configured)
    } else {
        env::current_dir()
            .map(|current| current.join(configured))
            .map_err(|error| {
                CompileError::report(
                    "ruddy-home-unavailable",
                    "could not resolve the relative `RUDDY_HOME` folder",
                )
                .with_note(error.to_string())
                .with_help("set `RUDDY_HOME` to an absolute writable folder")
            })
    }
}

fn git_cache(home: &Path) -> PathBuf {
    home.join("cache/git")
}

fn cache_error(message: impl Into<String>, error: impl ToString) -> CompileError {
    CompileError::report("git-cache-unavailable", message)
        .with_note(error.to_string())
        .with_help("check that the Git dependency cache is writable, then try again")
}

fn invalid_cache(path: &Path, detail: impl Into<String>) -> CompileError {
    CompileError::report(
        "git-cache-invalid",
        format!("cached Git dependency `{}` is not usable", path.display()),
    )
    .with_note(detail)
    .with_help("remove this cached folder and try again")
}

fn acquire_cache_lock(home: &Path) -> Result<File, CompileError> {
    let git = git_cache(home);
    fs::create_dir_all(&git).map_err(|error| {
        CompileError::report(
            "git-cache-unavailable",
            format!("could not create Git dependency cache `{}`", git.display()),
        )
        .with_note(error.to_string())
        .with_help("check that the cache folder is writable")
    })?;
    let path = git.join("cache.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            CompileError::report(
                "git-cache-unavailable",
                format!(
                    "could not open Git dependency cache lock `{}`",
                    path.display()
                ),
            )
            .with_note(error.to_string())
            .with_help("check that the cache folder is writable")
        })?;
    let locked = loop {
        ruddy::cancellation::checkpoint();
        match fs2::FileExt::try_lock_exclusive(&file) {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => break result,
        }
    };
    locked.map_err(|error| {
        CompileError::report(
            "git-cache-unavailable",
            format!(
                "could not lock the Git dependency cache `{}`",
                path.display()
            ),
        )
        .with_note(error.to_string())
        .with_help("wait for another Ruddy process to finish, then try again")
    })?;
    Ok(file)
}

fn clone_unlocked(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
) -> Result<(PathBuf, String), CompileError> {
    let temp = temporary_checkout(home, url, selector)?;
    clone_to(url, selector, None, temp.path())?;
    let repo = gix::open(temp.path()).map_err(|error| {
        CompileError::report(
            "git-fetch-failed",
            format!(
                "could not open downloaded Git dependency `{}`",
                crate::redact_git_url(url)
            ),
        )
        .with_note(redact_git_cause(error.to_string(), url))
    })?;
    let id = match selector {
        GitSelector::Rev(revision) => repo.rev_parse_single(revision).map_err(|error| {
            CompileError::report(
                "git-revision-not-found",
                format!(
                    "Git dependency `{}` has no revision `{revision}`",
                    crate::redact_git_url(url)
                ),
            )
            .with_note(redact_git_cause(error.to_string(), url))
            .with_help("check the `rev` value in `Ruddy.toml`")
        })?,
        _ => repo.head_id().map_err(|error| {
            CompileError::report(
                "git-revision-not-found",
                format!(
                    "Git dependency `{}` has no default revision",
                    crate::redact_git_url(url)
                ),
            )
            .with_note(redact_git_cause(error.to_string(), url))
            .with_help("select an existing `branch`, `tag`, or `rev`")
        })?,
    };
    let object = id.object().map_err(|error| {
        CompileError::report(
            "git-revision-unreadable",
            format!(
                "could not read the selected revision from `{}`",
                crate::redact_git_url(url)
            ),
        )
        .with_note(redact_git_cause(error.to_string(), url))
    })?;
    let commit = object
        .peel_to_commit()
        .map_err(|error| {
            CompileError::report(
                "git-revision-not-commit",
                format!(
                    "the selected revision from `{}` is not a commit",
                    crate::redact_git_url(url)
                ),
            )
            .with_note(redact_git_cause(error.to_string(), url))
            .with_help("select a branch, tag, or revision that points to a commit")
        })?
        .id
        .to_hex()
        .to_string();
    checkout_commit(&repo, &commit).map_err(|error| {
        CompileError::report(
            "git-checkout-failed",
            format!(
                "could not prepare Git dependency `{}` at `{commit}`",
                crate::redact_git_url(url)
            ),
        )
        .with_note(redact_git_cause(error.to_string(), url))
    })?;
    drop(repo);
    let final_path = checkout_path(home, url, selector, &commit);
    install_checkout(temp.path(), &final_path, &commit)?;
    Ok((final_path, commit))
}

fn clone_locked(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
    commit: &str,
) -> Result<PathBuf, CompileError> {
    let final_path = checkout_path(home, url, selector, commit);
    let temp = temporary_checkout(home, url, selector)?;
    clone_to(url, selector, Some(commit), temp.path())?;
    install_checkout(temp.path(), &final_path, commit)?;
    Ok(final_path)
}

fn clone_to(
    url: &str,
    selector: GitSelector<'_>,
    locked: Option<&str>,
    destination: &Path,
) -> Result<(), CompileError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CompileError::report(
                "git-cache-unavailable",
                format!(
                    "could not create Git dependency cache `{}`",
                    parent.display()
                ),
            )
            .with_note(error.to_string())
            .with_help("check that the cache folder is writable")
        })?;
    }
    let mut prepare = gix::prepare_clone(url, destination).map_err(|error| {
        CompileError::report(
            "git-fetch-failed",
            format!(
                "could not prepare Git dependency `{}` for download",
                crate::redact_git_url(url)
            ),
        )
        .with_note(redact_git_cause(error.to_string(), url))
    })?;
    if locked.is_none() {
        prepare = match selector {
            GitSelector::Branch(value) => prepare
                .with_ref_name(Some(format!("refs/heads/{value}").as_str()))
                .map_err(|error| {
                    CompileError::report(
                        "dependency-selector-invalid",
                        format!("`{value}` is not a valid Git branch name"),
                    )
                    .with_note(error.to_string())
                })?,
            GitSelector::Tag(value) => prepare
                .with_ref_name(Some(format!("refs/tags/{value}").as_str()))
                .map_err(|error| {
                    CompileError::report(
                        "dependency-selector-invalid",
                        format!("`{value}` is not a valid Git tag name"),
                    )
                    .with_note(error.to_string())
                })?,
            GitSelector::Default | GitSelector::Rev(_) => prepare,
        };
    }
    prepare = prepare.configure_remote(move |remote| {
        let effective = remote
            .url(gix::remote::Direction::Fetch)
            .ok_or("Git remote has no fetch URL")?;
        if effective.scheme != gix::url::Scheme::Https {
            return Err(
                "Git configuration rewrote the dependency URL to a non-HTTPS address".into(),
            );
        }
        // Fetch only normally advertised branch and tag refs. In particular,
        // never request a lock's raw object ID: many servers reject wants for
        // unadvertised objects. All tags are needed for tag-only revisions.
        Ok(remote.with_fetch_tags(gix::remote::fetch::Tags::All))
    });
    let cancellation = ruddy::cancellation::Cancellation::current();
    let result = (|| {
        let (mut checkout, _) =
            prepare.fetch_then_checkout(gix::progress::Discard, cancellation.signal())?;
        let (repo, _) = checkout.main_worktree(gix::progress::Discard, cancellation.signal())?;
        ruddy::cancellation::checkpoint();
        if let Some(commit) = locked {
            checkout_commit(&repo, commit)?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(destination);
        return Err(CompileError::report(
            "git-fetch-failed",
            format!(
                "could not fetch or check out Git dependency `{}`",
                crate::redact_git_url(url)
            ),
        )
        .with_note(redact_git_cause(error_chain(error.as_ref()), url))
        .with_help("check the URL, selected branch or tag, network access, and credentials"));
    }
    Ok(())
}

fn redact_git_cause(message: String, url: &str) -> String {
    message
        .replace(url, &crate::redact_git_url(url))
        .split_whitespace()
        .map(redact_url_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_url_word(word: &str) -> String {
    let Some(scheme_end) = word.find("://") else {
        return word.to_string();
    };
    let start = word[..scheme_end]
        .rfind(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
        })
        .map_or(0, |at| at + 1);
    let end = word
        .trim_end_matches(['`', '\'', '"', ',', ';', ':', ')', ']', '}'])
        .len();
    if end <= start {
        return word.to_string();
    }
    format!(
        "{}{}{}",
        &word[..start],
        crate::redact_git_url(&word[start..end]),
        &word[end..]
    )
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

fn install_checkout(temp: &Path, destination: &Path, commit: &str) -> Result<(), CompileError> {
    let parent = destination
        .parent()
        .ok_or_else(|| invalid_cache(destination, "the cache path has no parent folder"))?;
    fs::create_dir_all(parent).map_err(|error| {
        cache_error(
            format!(
                "could not create Git dependency cache `{}`",
                parent.display()
            ),
            error,
        )
    })?;
    if destination.exists() {
        if restore_checkout(destination, commit).is_ok() {
            fs::remove_dir_all(temp).ok();
            return Ok(());
        }
        fs::remove_dir_all(destination).map_err(|error| {
            cache_error(
                format!(
                    "could not replace cached Git dependency `{}`",
                    destination.display()
                ),
                error,
            )
        })?;
    }
    fs::rename(temp, destination).map_err(|error| {
        cache_error(
            format!(
                "could not install Git dependency in cache `{}`",
                destination.display()
            ),
            error,
        )
    })?;
    if let Err(error) = restore_checkout(destination, commit) {
        let _ = fs::remove_dir_all(destination);
        return Err(error);
    }
    Ok(())
}

fn restore_checkout(path: &Path, commit: &str) -> Result<(), CompileError> {
    let git_dir = path.join(".git");
    let metadata = fs::symlink_metadata(&git_dir).map_err(|error| {
        invalid_cache(
            path,
            format!("its `.git` folder could not be read: {error}"),
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid_cache(path, "its `.git` path is not a safe folder"));
    }
    let repo = gix::open(path)
        .map_err(|error| invalid_cache(path, format!("Git could not open it: {error}")))?;
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        cache_error(
            format!("could not open cached Git dependency `{}`", path.display()),
            error,
        )
    })?;
    let canonical_git = fs::canonicalize(&git_dir).map_err(|error| {
        cache_error(
            format!("could not open cached Git data `{}`", git_dir.display()),
            error,
        )
    })?;
    if repo.workdir() != Some(canonical_path.as_path())
        || repo.git_dir() != canonical_git.as_path()
        || !canonical_git.starts_with(&canonical_path)
    {
        return Err(invalid_cache(
            path,
            "its Git data points outside the cached dependency folder",
        ));
    }
    checkout_commit(&repo, commit).map_err(|error| {
        invalid_cache(
            path,
            format!("commit `{commit}` could not be restored: {error}"),
        )
    })?;
    if !path.join("Ruddy.toml").is_file() {
        return Err(CompileError::report(
            "git-project-missing-manifest",
            format!("Git dependency at commit `{commit}` is not a Ruddy project"),
        )
        .with_note(format!("`{}` contains no `Ruddy.toml`", path.display()))
        .with_help("choose a revision containing a Ruddy project, or add `Ruddy.toml` there"));
    }
    Ok(())
}

fn checkout_commit(repo: &gix::Repository, commit: &str) -> Result<(), Box<dyn std::error::Error>> {
    use gix::NestedProgress as _;
    let id = gix::hash::ObjectId::from_hex(commit.as_bytes())?;
    let commit = repo.find_object(id)?.peel_to_commit()?;
    let tree = commit.tree_id()?;
    let workdir = repo
        .workdir()
        .ok_or("downloaded Git dependency has no working folder")?;
    clear_worktree(workdir)?;
    let state = gix::index::State::from_tree(&tree, &repo.objects, Default::default())?;
    let mut index = gix::index::File::from_state(state, repo.index_path());
    let mut options =
        repo.checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)?;
    options.destination_is_initially_empty = true;
    let mut progress = gix::progress::Discard;
    let files = progress.add_child("checkout");
    let bytes = progress.add_child("bytes");
    let cancellation = ruddy::cancellation::Cancellation::current();
    gix::worktree::state::checkout(
        &mut index,
        workdir,
        repo.objects.clone().into_arc()?,
        &files,
        &bytes,
        cancellation.signal(),
        options,
    )?;
    // gix may report an interrupted checkout as Ok; do not publish its partial index.
    ruddy::cancellation::checkpoint();
    index.write(Default::default())?;
    repo.reference(
        "HEAD",
        commit.id,
        gix::refs::transaction::PreviousValue::Any,
        "ruddy checkout",
    )?;
    Ok(())
}

fn clear_worktree(workdir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(workdir)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if fs::symlink_metadata(&path)?.file_type().is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn validate_selector(selector: GitSelector<'_>) -> Result<(), &'static str> {
    match selector {
        GitSelector::Default => Ok(()),
        GitSelector::Rev(value) => (value.len() >= 7
            && value.len() <= 40
            && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(())
        .ok_or("a revision must contain 7 to 40 hexadecimal characters"),
        GitSelector::Branch("") | GitSelector::Tag("") => {
            Err("a Git branch or tag cannot be empty")
        }
        GitSelector::Branch(value) => gix::refs::FullName::try_from(format!("refs/heads/{value}"))
            .map(|_| ())
            .map_err(|_| "the selected branch is not a valid Git name"),
        GitSelector::Tag(value) => gix::refs::FullName::try_from(format!("refs/tags/{value}"))
            .map(|_| ())
            .map_err(|_| "the selected tag is not a valid Git name"),
    }
}

fn validate_full_commit(commit: &str) -> Result<(), &'static str> {
    (commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(())
        .ok_or("a locked commit must contain exactly 40 hexadecimal characters")
}

fn cache_key(url: &str, selector: GitSelector<'_>) -> String {
    format!(
        "{:016x}",
        hash(&(url.to_owned() + &format!("{selector:?}")))
    )
}

fn checkout_path(home: &Path, url: &str, selector: GitSelector<'_>, commit: &str) -> PathBuf {
    git_cache(home)
        .join("checkouts")
        .join(cache_key(url, selector))
        .join(commit.to_ascii_lowercase())
}

static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TemporaryCheckout(PathBuf);

impl TemporaryCheckout {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryCheckout {
    fn drop(&mut self) {
        let _ = remove_cache_entry(&self.0);
    }
}

fn temporary_checkout(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
) -> Result<TemporaryCheckout, CompileError> {
    let directory = git_cache(home).join("tmp");
    clean_stale_temporary_checkouts(home, url, selector)?;
    let prefix = format!("{}-", cache_key(url, selector));
    for _ in 0..100 {
        let path = directory.join(format!(
            "{}{}-{}",
            prefix,
            std::process::id(),
            TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if !path.exists() {
            return Ok(TemporaryCheckout(path));
        }
    }
    Err(CompileError::report(
        "git-cache-unavailable",
        format!(
            "could not reserve temporary space in Git dependency cache `{}`",
            directory.display()
        ),
    )
    .with_help("remove stale files from the cache or choose another `RUDDY_HOME`"))
}

fn clean_stale_temporary_checkouts(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
) -> Result<(), CompileError> {
    let directory = git_cache(home).join("tmp");
    fs::create_dir_all(&directory).map_err(|error| {
        cache_error(
            format!(
                "could not create temporary Git cache folder `{}`",
                directory.display()
            ),
            error,
        )
    })?;
    let prefix = format!("{}-", cache_key(url, selector));
    for entry in fs::read_dir(&directory).map_err(|error| {
        cache_error(
            format!(
                "could not inspect temporary Git cache folder `{}`",
                directory.display()
            ),
            error,
        )
    })? {
        let path = entry
            .map_err(|error| {
                cache_error(
                    format!(
                        "could not inspect temporary Git cache folder `{}`",
                        directory.display()
                    ),
                    error,
                )
            })?
            .path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix))
        {
            remove_cache_entry(&path).map_err(|error| {
                cache_error(
                    format!(
                        "could not remove stale Git cache entry `{}`",
                        path.display()
                    ),
                    error,
                )
            })?;
        }
    }
    Ok(())
}

fn remove_cache_entry(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
