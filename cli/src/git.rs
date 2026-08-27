use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
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
            validate_commit(&entry.commit).map_err(|message| {
                CompileError::one(format!(
                    "invalid lockfile entry for {}: {message}",
                    entry.url
                ))
            })?;
            let selector = selector_from_locked(&entry.selector)?;
            if matches!(
                selector,
                GitSelector::Branch("") | GitSelector::Tag("") | GitSelector::Rev("")
            ) {
                return Err(CompileError::one(format!(
                    "invalid lockfile entry for {}: selector must not be empty",
                    entry.url
                )));
            }
            if let GitSelector::Rev(revision) = selector
                && !revision.eq_ignore_ascii_case(&entry.commit)
            {
                return Err(CompileError::one(format!(
                    "invalid lockfile entry for {}: exact revision `{revision}` does not match commit `{}`",
                    entry.url, entry.commit
                )));
            }
            let key = (entry.url, LockedSelectorKey::new(selector));
            if locked.insert(key, entry.commit).is_some() {
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
        })
    }

    pub(crate) fn resolve(&mut self, spec: &DependencySpec) -> Result<PathBuf, CompileError> {
        let url = spec.git().expect("called for a Git dependency");
        let selector = spec.selector()?;
        if self.home.is_none() {
            self.home = Some(ruddy_home()?);
        }
        let home = self.home.as_deref().expect("initialized above");
        let key = (url.to_string(), LockedSelectorKey::new(selector));
        let locked = self.locked.get(&key).cloned();
        if let Some(commit) = locked.as_deref() {
            let checkout = checkout_path(home, url, selector, commit);
            if valid_checkout(&checkout, commit) {
                self.resolved.insert(key, commit.into());
                return Ok(checkout);
            }
            if checkout.exists() {
                fs::remove_dir_all(&checkout).map_err(|error| {
                    CompileError::one(format!(
                        "could not replace invalid Git cache checkout {}: {error}",
                        checkout.display()
                    ))
                })?;
            }
            let checkout = clone_commit(home, url, selector, Some(commit))?;
            self.resolved.insert(key, commit.into());
            return Ok(checkout);
        }
        let (checkout, commit) = clone_unlocked(home, url, selector)?;
        self.resolved.insert(key, commit);
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
        atomic_write(&self.lock_path, source.as_bytes()).map_err(|error| {
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
    let commit = repo
        .head_id()
        .map_err(|e| {
            CompileError::one(format!(
                "Git dependency `{url}` has no resolved commit: {e}"
            ))
        })?
        .to_hex()
        .to_string();
    let final_path = checkout_path(home, url, selector, &commit);
    install_checkout(&temp, &final_path, &commit)?;
    Ok((final_path, commit))
}
fn clone_commit(
    home: &Path,
    url: &str,
    selector: GitSelector<'_>,
    commit: Option<&str>,
) -> Result<PathBuf, CompileError> {
    let commit = commit.expect("locked commit");
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
    let _ = fs::remove_dir_all(destination);
    let mut prepare = gix::prepare_clone(url, destination)
        .map_err(|e| CompileError::one(format!("could not prepare Git dependency `{url}`: {e}")))?;
    let tag_ref = match selector {
        GitSelector::Tag(value) => Some(format!("refs/tags/{value}")),
        _ => None,
    };
    prepare = match selector {
        GitSelector::Branch(value) => prepare
            .with_ref_name(Some(value))
            .map_err(|e| CompileError::one(format!("invalid branch `{value}`: {e}")))?,
        GitSelector::Tag(value) => prepare
            .with_ref_name(tag_ref.as_deref())
            .map_err(|e| CompileError::one(format!("invalid tag `{value}`: {e}")))?,
        _ => prepare,
    };
    let requested_commit = locked.or(match selector {
        GitSelector::Rev(value) => Some(value),
        _ => None,
    });
    if let Some(revision) = requested_commit {
        validate_commit(revision).map_err(|message| {
            CompileError::one(format!("invalid revision `{revision}`: {message}"))
        })?;
        let revision = revision.to_string();
        prepare = prepare.configure_remote(move |remote| {
            Ok(remote.with_refspecs([revision.as_str()], gix::remote::Direction::Fetch)?)
        });
    }
    let result = (|| {
        let (mut checkout, _) =
            prepare.fetch_then_checkout(gix::progress::Discard, &AtomicBool::new(false))?;
        let (repo, _) = checkout.main_worktree(gix::progress::Discard, &AtomicBool::new(false))?;
        if let Some(commit) = requested_commit {
            checkout_commit(&repo, commit)?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(destination);
        return Err(CompileError::one(format!(
            "could not fetch or check out Git dependency `{url}`: {error}"
        )));
    }
    Ok(())
}
fn install_checkout(temp: &Path, destination: &Path, commit: &str) -> Result<(), CompileError> {
    if destination.exists() {
        if valid_checkout(destination, commit) {
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
    })
}
fn checkout_commit(repo: &gix::Repository, commit: &str) -> Result<(), Box<dyn std::error::Error>> {
    use gix::NestedProgress as _;
    let id = gix::hash::ObjectId::from_hex(commit.as_bytes())?;
    let tree = repo.find_object(id)?.peel_to_tree()?.id;
    let workdir = repo.workdir().ok_or("Git dependency has no worktree")?;
    for entry in fs::read_dir(workdir)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
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
        id,
        gix::refs::transaction::PreviousValue::Any,
        "ruddy checkout",
    )?;
    Ok(())
}
fn valid_checkout(path: &Path, commit: &str) -> bool {
    gix::open(path)
        .ok()
        .and_then(|r| r.head_id().ok().map(|id| id.to_hex().to_string()))
        .is_some_and(|id| id == commit)
        && path.join("Ruddy.toml").is_file()
}
fn validate_commit(commit: &str) -> Result<(), &'static str> {
    (commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(())
        .ok_or("commit must be a full 40-digit object ID")
}
fn checkout_path(home: &Path, url: &str, selector: GitSelector<'_>, commit: &str) -> PathBuf {
    home.join("git/checkouts")
        .join(format!(
            "{:016x}",
            hash(&(url.to_owned() + &format!("{:?}", selector)))
        ))
        .join(commit)
}
fn temporary_path(home: &Path, url: &str, selector: GitSelector<'_>) -> PathBuf {
    home.join("git/tmp").join(format!(
        "{:016x}-{}",
        hash(&(url.to_owned() + &format!("{:?}", selector))),
        std::process::id()
    ))
}
fn hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("Ruddy.lock");
    for i in 0..100 {
        let tmp = parent.join(format!(".{name}.tmp-{}-{i}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(contents).and_then(|_| f.sync_all()) {
                    fs::remove_file(&tmp).ok();
                    return Err(e);
                }
                if let Err(e) = fs::rename(&tmp, path) {
                    fs::remove_file(&tmp).ok();
                    return Err(e);
                }
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no temporary lockfile name available",
    ))
}
