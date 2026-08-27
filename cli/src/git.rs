use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{CompileError, DependencySpec, GitSelector};

pub const LOCKFILE: &str = "Ruddy.lock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub version: u32,
    #[serde(default, rename = "git")]
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
    // A checkout is restored while holding this marker and remains protected
    // until compilation has finished reading it.
    cache_locks: Vec<gix_lock::Marker>,
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
                CompileError::one(format!(
                    "could not parse lockfile {}: {error}",
                    lock_path.display()
                ))
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(CompileError::one(format!(
                    "could not read lockfile {}: {error}",
                    lock_path.display()
                )));
            }
        };
        if lock.as_ref().is_some_and(|lock| lock.version != 1) {
            return Err(CompileError::one(format!(
                "lockfile {} has unsupported version",
                lock_path.display()
            )));
        }
        let mut locked = BTreeMap::new();
        for entry in lock.into_iter().flat_map(|lock| lock.entries) {
            if !entry.url.starts_with("https://") {
                return Err(CompileError::one(format!(
                    "invalid lockfile entry URL `{}`: Git sources must use HTTPS",
                    entry.url
                )));
            }
            validate_full_commit(&entry.commit).map_err(|message| {
                CompileError::one(format!(
                    "invalid lockfile entry for {}: {message}",
                    entry.url
                ))
            })?;
            let selector = selector_from_locked(&entry.selector)?;
            validate_selector(selector).map_err(|message| {
                CompileError::one(format!(
                    "invalid lockfile entry for {}: {message}",
                    entry.url
                ))
            })?;
            if let GitSelector::Rev(revision) = selector
                && !entry
                    .commit
                    .to_ascii_lowercase()
                    .starts_with(&revision.to_ascii_lowercase())
            {
                return Err(CompileError::one(format!(
                    "invalid lockfile entry for {}: exact revision `{revision}` is not a prefix of commit `{}`",
                    entry.url, entry.commit
                )));
            }
            let key = (entry.url, LockedSelectorKey::new(selector));
            if locked
                .insert(key, entry.commit.to_ascii_lowercase())
                .is_some()
            {
                return Err(CompileError::one(
                    "lockfile contains a duplicate Git source selector",
                ));
            }
        }
        Ok(Self {
            home: None,
            lock_path,
            locked,
            resolved: BTreeMap::new(),
            cache_locks: Vec::new(),
        })
    }

    pub(crate) fn resolve(&mut self, spec: &DependencySpec) -> Result<PathBuf, CompileError> {
        let url = spec.git().expect("called for a Git dependency");
        let selector = spec.selector()?;
        validate_selector(selector).map_err(CompileError::one)?;
        if self.home.is_none() {
            self.home = Some(ruddy_home()?);
        }
        let home = self.home.as_deref().expect("initialized above");
        let key = (url.to_string(), LockedSelectorKey::new(selector));
        if let Some(commit) = self.resolved.get(&key) {
            return Ok(checkout_path(home, url, selector, commit));
        }
        let marker = acquire_cache_lock(home, url, selector)?;
        let locked = self.locked.get(&key).cloned();
        let (checkout, commit) = if let Some(commit) = locked {
            let checkout = checkout_path(home, url, selector, &commit);
            if checkout.exists() && restore_checkout(&checkout, &commit).is_ok() {
                (checkout, commit)
            } else {
                if checkout.exists() {
                    fs::remove_dir_all(&checkout).map_err(|error| {
                        CompileError::one(format!(
                            "could not replace invalid Git cache checkout {}: {error}",
                            checkout.display()
                        ))
                    })?;
                }
                (clone_locked(home, url, selector, &commit)?, commit)
            }
        } else {
            clone_unlocked(home, url, selector)?
        };
        self.resolved.insert(key, commit);
        self.cache_locks.push(marker);
        Ok(checkout)
    }

    pub(crate) fn write_if_changed(&self) -> Result<(), CompileError> {
        if self.resolved.is_empty() {
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
        .map_err(|error| CompileError::one(format!("could not serialize lockfile: {error}")))?;
        if fs::read_to_string(&self.lock_path).ok().as_deref() == Some(&source) {
            return Ok(());
        }
        crate::replace_file(&self.lock_path, source.as_bytes()).map_err(|error| {
            CompileError::one(format!(
                "could not write lockfile {}: {error}",
                self.lock_path.display()
            ))
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
        return Err(CompileError::one(
            "lockfile Git entry has conflicting selectors",
        ));
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

pub fn ruddy_home() -> Result<PathBuf, CompileError> {
    if let Some(path) = env::var_os("RUDDY_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("ruddy"));
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".cache/ruddy"))
        .ok_or_else(|| {
            CompileError::one("could not determine Ruddy cache directory; set RUDDY_HOME")
        })
}

fn acquire_cache_lock(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
) -> Result<gix_lock::Marker, CompileError> {
    let locks = home.join("git/locks");
    let resource = locks.join(cache_key(url, selector));
    gix_lock::Marker::acquire_to_hold_resource(
        &resource,
        gix_lock::acquire::Fail::AfterDurationWithBackoff(Duration::from_secs(120)),
        Some(home.join("git")),
    )
    .map_err(|error| {
        CompileError::one(format!(
            "could not lock Git cache entry {}: {error}",
            resource.display()
        ))
    })
}

fn clone_unlocked(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
) -> Result<(PathBuf, String), CompileError> {
    let temp = temporary_path(home, url, selector);
    clone_to(url, selector, None, &temp)?;
    let repo = gix::open(&temp).map_err(|e| {
        CompileError::one(format!(
            "could not open fetched Git dependency `{url}`: {e}"
        ))
    })?;
    let id = match selector {
        GitSelector::Rev(revision) => repo.rev_parse_single(revision).map_err(|error| {
            CompileError::one(format!(
                "could not resolve Git revision `{revision}` from `{url}`: {error}"
            ))
        })?,
        _ => repo.head_id().map_err(|e| {
            CompileError::one(format!(
                "Git dependency `{url}` has no resolved commit: {e}"
            ))
        })?,
    };
    let object = id.object().map_err(|error| {
        CompileError::one(format!(
            "could not read resolved Git object for `{url}`: {error}"
        ))
    })?;
    let commit = object
        .peel_to_commit()
        .map_err(|error| {
            CompileError::one(format!(
                "Git dependency `{url}` did not resolve to a commit: {error}"
            ))
        })?
        .id
        .to_hex()
        .to_string();
    checkout_commit(&repo, &commit).map_err(|error| {
        CompileError::one(format!(
            "could not check out Git dependency `{url}` at `{commit}`: {error}"
        ))
    })?;
    drop(repo);
    let final_path = checkout_path(home, url, selector, &commit);
    install_checkout(&temp, &final_path, &commit)?;
    Ok((final_path, commit))
}

fn clone_locked(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
    commit: &str,
) -> Result<PathBuf, CompileError> {
    let final_path = checkout_path(home, url, selector, commit);
    let temp = temporary_path(home, url, selector);
    clone_to(url, selector, Some(commit), &temp)?;
    install_checkout(&temp, &final_path, commit)?;
    Ok(final_path)
}

fn clone_to(
    url: &str,
    selector: GitSelector<'_>,
    locked: Option<&str>,
    destination: &Path,
) -> Result<(), CompileError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            CompileError::one(format!(
                "could not create Git cache {}: {e}",
                parent.display()
            ))
        })?;
    }
    let mut prepare = gix::prepare_clone(url, destination)
        .map_err(|e| CompileError::one(format!("could not prepare Git dependency `{url}`: {e}")))?;
    if locked.is_none() {
        prepare = match selector {
            GitSelector::Branch(value) => prepare
                .with_ref_name(Some(format!("refs/heads/{value}").as_str()))
                .map_err(|e| CompileError::one(format!("invalid branch `{value}`: {e}")))?,
            GitSelector::Tag(value) => prepare
                .with_ref_name(Some(format!("refs/tags/{value}").as_str()))
                .map_err(|e| CompileError::one(format!("invalid tag `{value}`: {e}")))?,
            GitSelector::Default | GitSelector::Rev(_) => prepare,
        };
    }
    let requested_commit = locked.map(str::to_owned);
    prepare = prepare.configure_remote(move |remote| {
        let effective = remote
            .url(gix::remote::Direction::Fetch)
            .ok_or("Git remote has no fetch URL")?;
        if effective.scheme != gix::url::Scheme::Https {
            return Err(format!(
                "effective Git remote URL must use HTTPS after configuration rewriting, found `{effective}`"
            )
            .into());
        }
        if let Some(commit) = requested_commit.as_deref() {
            Ok(remote.with_refspecs([commit], gix::remote::Direction::Fetch)?)
        } else {
            Ok(remote)
        }
    });
    let result = (|| {
        let (mut checkout, _) =
            prepare.fetch_then_checkout(gix::progress::Discard, &AtomicBool::new(false))?;
        let (repo, _) = checkout.main_worktree(gix::progress::Discard, &AtomicBool::new(false))?;
        if let Some(commit) = locked {
            checkout_commit(&repo, commit)?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(destination);
        return Err(CompileError::one(format!(
            "could not fetch or check out Git dependency `{url}`: {}",
            error_chain(error.as_ref())
        )));
    }
    Ok(())
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
    let parent = destination.parent().ok_or_else(|| {
        CompileError::one(format!(
            "Git cache checkout {} has no parent directory",
            destination.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CompileError::one(format!(
            "could not create Git checkout directory {}: {error}",
            parent.display()
        ))
    })?;
    if destination.exists() {
        if restore_checkout(destination, commit).is_ok() {
            fs::remove_dir_all(temp).ok();
            return Ok(());
        }
        fs::remove_dir_all(destination).map_err(|error| {
            CompileError::one(format!(
                "could not replace invalid Git cache checkout {}: {error}",
                destination.display()
            ))
        })?;
    }
    fs::rename(temp, destination).map_err(|e| {
        CompileError::one(format!(
            "could not install Git checkout {}: {e}",
            destination.display()
        ))
    })?;
    restore_checkout(destination, commit)
}

fn restore_checkout(path: &Path, commit: &str) -> Result<(), CompileError> {
    let git_dir = path.join(".git");
    let metadata = fs::symlink_metadata(&git_dir).map_err(|error| {
        CompileError::one(format!(
            "cached Git checkout {} has no safe repository directory: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(CompileError::one(format!(
            "cached Git checkout {} has an unsafe repository directory",
            path.display()
        )));
    }
    let repo = gix::open(path).map_err(|error| {
        CompileError::one(format!(
            "could not open cached Git checkout {}: {error}",
            path.display()
        ))
    })?;
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        CompileError::one(format!(
            "could not resolve cached Git checkout {}: {error}",
            path.display()
        ))
    })?;
    let canonical_git = fs::canonicalize(&git_dir).map_err(|error| {
        CompileError::one(format!(
            "could not resolve cached Git repository {}: {error}",
            git_dir.display()
        ))
    })?;
    if repo.workdir() != Some(canonical_path.as_path())
        || repo.git_dir() != canonical_git.as_path()
        || !canonical_git.starts_with(&canonical_path)
    {
        return Err(CompileError::one(format!(
            "cached Git checkout {} points outside its cache directory",
            path.display()
        )));
    }
    checkout_commit(&repo, commit).map_err(|error| {
        CompileError::one(format!(
            "could not restore cached Git checkout {} at `{commit}`: {error}",
            path.display()
        ))
    })?;
    if !path.join("Ruddy.toml").is_file() {
        return Err(CompileError::one(format!(
            "cached Git checkout {} contains no Ruddy.toml at `{commit}`",
            path.display()
        )));
    }
    Ok(())
}

fn checkout_commit(repo: &gix::Repository, commit: &str) -> Result<(), Box<dyn std::error::Error>> {
    use gix::NestedProgress as _;
    let id = gix::hash::ObjectId::from_hex(commit.as_bytes())?;
    let commit = repo.find_object(id)?.peel_to_commit()?;
    let tree = commit.tree_id()?;
    let workdir = repo.workdir().ok_or("Git dependency has no worktree")?;
    clear_worktree(workdir)?;
    let state = gix::index::State::from_tree(&tree, &repo.objects, Default::default())?;
    let mut index = gix::index::File::from_state(state, repo.index_path());
    let mut options =
        repo.checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)?;
    options.destination_is_initially_empty = true;
    let mut progress = gix::progress::Discard;
    let files = progress.add_child("checkout");
    let bytes = progress.add_child("bytes");
    gix::worktree::state::checkout(
        &mut index,
        workdir,
        repo.objects.clone().into_arc()?,
        &files,
        &bytes,
        &AtomicBool::new(false),
        options,
    )?;
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
        .ok_or("revision must contain 7 to 40 hexadecimal digits"),
        GitSelector::Branch("") | GitSelector::Tag("") => Err("selector must not be empty"),
        GitSelector::Branch(value) => gix::refs::FullName::try_from(format!("refs/heads/{value}"))
            .map(|_| ())
            .map_err(|_| "branch and tag selectors must be valid Git reference names"),
        GitSelector::Tag(value) => gix::refs::FullName::try_from(format!("refs/tags/{value}"))
            .map(|_| ())
            .map_err(|_| "branch and tag selectors must be valid Git reference names"),
    }
}

fn validate_full_commit(commit: &str) -> Result<(), &'static str> {
    (commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(())
        .ok_or("commit must be a full 40-digit object ID")
}

fn cache_key(url: &str, selector: GitSelector<'_>) -> String {
    format!(
        "{:016x}",
        hash(&(url.to_owned() + &format!("{selector:?}")))
    )
}

fn checkout_path(home: &Path, url: &str, selector: GitSelector<'_>, commit: &str) -> PathBuf {
    home.join("git/checkouts")
        .join(cache_key(url, selector))
        .join(commit.to_ascii_lowercase())
}

static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temporary_path(home: &Path, url: &str, selector: GitSelector<'_>) -> PathBuf {
    home.join("git/tmp").join(format!(
        "{}-{}-{}",
        cache_key(url, selector),
        std::process::id(),
        TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
