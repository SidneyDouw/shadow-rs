use crate::build::{ConstType, ConstVal, ShadowConst};
use crate::ci::CiType;
use crate::date_time::DateTime;
use crate::err::*;
use crate::Format;
use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

pub const BRANCH: ShadowConst = "BRANCH";
pub const TAG: ShadowConst = "TAG";
pub const SHORT_COMMIT: ShadowConst = "SHORT_COMMIT";
pub const COMMIT_HASH: ShadowConst = "COMMIT_HASH";
pub const COMMIT_DATE: ShadowConst = "COMMIT_DATE";
pub const COMMIT_DATE_2822: ShadowConst = "COMMIT_DATE_2822";
pub const COMMIT_DATE_3339: ShadowConst = "COMMIT_DATE_3339";
pub const COMMIT_AUTHOR: ShadowConst = "COMMIT_AUTHOR";
pub const COMMIT_EMAIL: ShadowConst = "COMMIT_EMAIL";
pub const GIT_CLEAN: ShadowConst = "GIT_CLEAN";
pub const GIT_STATUS_FILE: ShadowConst = "GIT_STATUS_FILE";

#[derive(Default, Debug)]
pub struct Git {
    map: BTreeMap<ShadowConst, ConstVal>,
    ci_type: CiType,
}

impl Git {
    fn update_str(&mut self, c: ShadowConst, v: String) {
        if let Some(val) = self.map.get_mut(c) {
            *val = ConstVal {
                desc: val.desc.clone(),
                v,
                t: ConstType::Str,
            }
        }
    }

    fn update_bool(&mut self, c: ShadowConst, v: bool) {
        if let Some(val) = self.map.get_mut(c) {
            *val = ConstVal {
                desc: val.desc.clone(),
                v: v.to_string(),
                t: ConstType::Bool,
            }
        }
    }

    fn init(&mut self, path: &Path, std_env: &BTreeMap<String, String>) -> SdResult<()> {
        // check git status
        let x = command_git_clean();
        self.update_bool(GIT_CLEAN, x);

        let x = command_git_status_file();
        self.update_str(GIT_STATUS_FILE, x);

        #[cfg(all(feature = "git2", not(feature = "gitoxide")))]
        self.init_git2(path)?;
        #[cfg(feature = "gitoxide")]
        self.init_gitoxide(path)?;

        // use command branch
        if let Some(x) = command_current_branch() {
            self.update_str(BRANCH, x)
        };

        // use command tag
        if let Some(x) = command_current_tag() {
            self.update_str(TAG, x)
        }

        // try use ci branch,tag
        self.ci_branch_tag(std_env);
        Ok(())
    }

    #[cfg(feature = "git2")]
    #[allow(dead_code)]
    fn init_git2(&mut self, path: &Path) -> SdResult<()> {
        use crate::git::git2_mod::git_repo;

        let repo = git_repo(path).map_err(ShadowError::new)?;
        let reference = repo.head().map_err(ShadowError::new)?;

        //get branch
        let branch = reference
            .shorthand()
            .map(|x| x.trim().to_string())
            .or_else(command_current_branch)
            .unwrap_or_default();

        //get HEAD branch
        let tag = command_current_tag().unwrap_or_default();

        self.update_str(BRANCH, branch);
        self.update_str(TAG, tag);
        if let Some(v) = reference.target() {
            let commit = v.to_string();
            self.update_str(COMMIT_HASH, commit.clone());
            let mut short_commit = commit.as_str();

            if commit.len() > 8 {
                short_commit = short_commit.get(0..8).unwrap();
            }
            self.update_str(SHORT_COMMIT, short_commit.to_string());
        }

        let commit = reference.peel_to_commit().map_err(ShadowError::new)?;

        let time_stamp = commit.time().seconds().to_string().parse::<i64>()?;
        let date_time = DateTime::timestamp_2_utc(time_stamp);
        self.update_str(COMMIT_DATE, date_time.human_format());

        self.update_str(COMMIT_DATE_2822, date_time.to_rfc2822());

        self.update_str(COMMIT_DATE_3339, date_time.to_rfc3339());

        let author = commit.author();
        if let Some(v) = author.email() {
            self.update_str(COMMIT_EMAIL, v.to_string());
        }

        if let Some(v) = author.name() {
            self.update_str(COMMIT_AUTHOR, v.to_string());
        }
        let status_file = Self::git2_dirty_stage(&repo);
        if status_file.trim().is_empty() {
            self.update_bool(GIT_CLEAN, true);
        } else {
            self.update_bool(GIT_CLEAN, false);
        }
        self.update_str(GIT_STATUS_FILE, status_file);

        Ok(())
    }

    #[cfg(feature = "gitoxide")]
    fn init_gitoxide(&mut self, path: &Path) -> SdResult<()> {
        let repo = gix::discover(path).map_err(ShadowError::new)?;

        let branch = repo
            .head_name()
            .map_err(ShadowError::new)?
            .map(|n| n.shorten().to_string())
            .unwrap_or("HEAD".to_string());

        let refs = repo.references().map_err(ShadowError::new)?;
        let tag = refs
            .tags()
            .map_err(ShadowError::new)?
            .filter_map(|tag| {
                let mut tag = tag.ok()?;

                let peeled_tag_id = match tag.inner.peeled {
                    Some(id) => id,
                    None => {
                        tag.peel_to_id_in_place().ok()?;
                        tag.inner.peeled.expect("peeled")
                    }
                };

                if peeled_tag_id != repo.head_id().ok()? {
                    None
                } else {
                    Some(tag)
                }
            })
            .map(|tag| tag.name().shorten().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        self.update_str(BRANCH, branch);
        self.update_str(TAG, tag);

        let commit = repo.head_commit().map_err(ShadowError::new)?;

        self.update_str(COMMIT_HASH, commit.id().to_string());
        self.update_str(
            SHORT_COMMIT,
            commit
                .id()
                .shorten()
                .map(|p| p.to_string())
                .unwrap_or_default(),
        );

        if let Ok(time) = commit.time() {
            let date_time = DateTime::timestamp_2_utc(time.seconds().into());

            self.update_str(COMMIT_DATE, date_time.human_format());
            self.update_str(COMMIT_DATE_2822, date_time.to_rfc2822());
            self.update_str(COMMIT_DATE_3339, date_time.to_rfc3339());
        }

        if let Ok(author) = commit.author() {
            let (name, email) = author.actor();
            self.update_str(COMMIT_AUTHOR, name.to_string());
            self.update_str(COMMIT_EMAIL, email.to_string());
        }

        Ok(())
    }

    //use git2 crates git repository 'dirty or stage' status files.
    #[cfg(feature = "git2")]
    pub fn git2_dirty_stage(repo: &git2::Repository) -> String {
        let mut repo_opts = git2::StatusOptions::new();
        repo_opts.include_ignored(false);
        if let Ok(statue) = repo.statuses(Some(&mut repo_opts)) {
            let mut dirty_files = Vec::new();
            let mut staged_files = Vec::new();

            for status in statue.iter() {
                if let Some(path) = status.path() {
                    match status.status() {
                        git2::Status::CURRENT => (),
                        git2::Status::INDEX_NEW
                        | git2::Status::INDEX_MODIFIED
                        | git2::Status::INDEX_DELETED
                        | git2::Status::INDEX_RENAMED
                        | git2::Status::INDEX_TYPECHANGE => staged_files.push(path.to_string()),
                        _ => dirty_files.push(path.to_string()),
                    };
                }
            }
            filter_git_dirty_stage(dirty_files, staged_files)
        } else {
            "".into()
        }
    }

    #[allow(clippy::manual_strip)]
    fn ci_branch_tag(&mut self, std_env: &BTreeMap<String, String>) {
        let mut branch: Option<String> = None;
        let mut tag: Option<String> = None;
        match self.ci_type {
            CiType::Gitlab => {
                if let Some(v) = std_env.get("CI_COMMIT_TAG") {
                    tag = Some(v.to_string());
                } else if let Some(v) = std_env.get("CI_COMMIT_REF_NAME") {
                    branch = Some(v.to_string());
                }
            }
            CiType::Github => {
                if let Some(v) = std_env.get("GITHUB_REF") {
                    let ref_branch_prefix: &str = "refs/heads/";
                    let ref_tag_prefix: &str = "refs/tags/";

                    if v.starts_with(ref_branch_prefix) {
                        branch = Some(
                            v.get(ref_branch_prefix.len()..)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    } else if v.starts_with(ref_tag_prefix) {
                        tag = Some(
                            v.get(ref_tag_prefix.len()..)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    }
                }
            }
            _ => {}
        }
        if let Some(x) = branch {
            self.update_str(BRANCH, x);
        }

        if let Some(x) = tag {
            self.update_str(TAG, x);
        }
    }
}

pub fn new_git(
    path: &Path,
    ci: CiType,
    std_env: &BTreeMap<String, String>,
) -> BTreeMap<ShadowConst, ConstVal> {
    let mut git = new_empty_git(ci);

    if let Err(e) = git.init(path, std_env) {
        println!("{e}");
    }

    git.map
}

fn new_empty_git(ci: CiType) -> Git {
    let mut git = Git {
        map: Default::default(),
        ci_type: ci,
    };
    git.map
        .insert(BRANCH, ConstVal::new("display current branch"));

    git.map.insert(TAG, ConstVal::new("display current tag"));

    git.map
        .insert(COMMIT_HASH, ConstVal::new("display current commit_id"));

    git.map.insert(
        SHORT_COMMIT,
        ConstVal::new("display current short commit_id"),
    );

    git.map.insert(
        COMMIT_AUTHOR,
        ConstVal::new("display current commit author"),
    );
    git.map
        .insert(COMMIT_EMAIL, ConstVal::new("display current commit email"));
    git.map
        .insert(COMMIT_DATE, ConstVal::new("display current commit date"));

    git.map.insert(
        COMMIT_DATE_2822,
        ConstVal::new("display current commit date by rfc2822"),
    );

    git.map.insert(
        COMMIT_DATE_3339,
        ConstVal::new("display current commit date by rfc3339"),
    );

    git.map.insert(
        GIT_CLEAN,
        ConstVal::new_bool("display current git repository status clean:'true or false'"),
    );

    git.map.insert(
        GIT_STATUS_FILE,
        ConstVal::new("display current git repository status files:'dirty or stage'"),
    );

    git
}

#[cfg(feature = "git2")]
pub mod git2_mod {
    use git2::Error as git2Error;
    use git2::Repository;
    use std::path::Path;

    pub fn git_repo<P: AsRef<Path>>(path: P) -> Result<Repository, git2Error> {
        git2::Repository::discover(path)
    }

    pub fn git2_current_branch(repo: &Repository) -> Option<String> {
        repo.head()
            .map(|x| x.shorthand().map(|x| x.to_string()))
            .unwrap_or(None)
    }
}

/// get current repository git branch.
///
/// When current repository exists git folder.
///
/// It's use default feature.This function try use [git2] crates get current branch.
/// If not use git2 feature,then try use [Command] to get.
pub fn branch() -> String {
    #[cfg(all(feature = "git2", not(feature = "gitoxide")))]
    {
        use crate::git::git2_mod::{git2_current_branch, git_repo};
        git_repo(".")
            .map(|x| git2_current_branch(&x))
            .unwrap_or_else(|_| command_current_branch())
            .unwrap_or_default()
    }
    #[cfg(feature = "gitoxide")]
    {
        match gix::discover(".") {
            Ok(repo) => match repo.head_name() {
                Ok(Some(name)) => name.shorten().to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    }
    #[cfg(all(not(feature = "git2"), not(feature = "gitoxide")))]
    {
        command_current_branch().unwrap_or_default()
    }
}

/// get current repository git tag.
///
/// When current repository exists git folder.
/// I's use [Command] to get.
pub fn tag() -> String {
    #[cfg(feature = "gitoxide")]
    {
        todo!()
    }
    #[cfg(not(feature = "gitoxide"))]
    command_current_tag().unwrap_or_default()
}

/// Check current git Repository status without nothing(dirty or stage)
///
/// if nothing,It means clean:true. On the contrary, it is 'dirty':false
pub fn git_clean() -> bool {
    #[cfg(all(feature = "git2", not(feature = "gitoxide")))]
    {
        use crate::git::git2_mod::git_repo;
        git_repo(".")
            .map(|x| Git::git2_dirty_stage(&x))
            .map(|x| x.trim().is_empty())
            .unwrap_or(true)
    }
    #[cfg(feature = "gitoxide")]
    {
        todo!()
    }
    #[cfg(all(not(feature = "git2"), not(feature = "gitoxide")))]
    {
        command_git_clean()
    }
}

/// List current git Repository statue(dirty or stage) contain file changed
///
/// Refer to the 'cargo fix' result output when git statue(dirty or stage) changed.
///
/// Example output:`   * examples/builtin_fn.rs (dirty)`
pub fn git_status_file() -> String {
    #[cfg(all(feature = "git2", not(feature = "gitoxide")))]
    {
        use crate::git::git2_mod::git_repo;
        git_repo(".")
            .map(|x| Git::git2_dirty_stage(&x))
            .unwrap_or_default()
    }
    #[cfg(feature = "gitoxide")]
    {
        todo!()
    }
    #[cfg(all(not(feature = "git2"), not(feature = "gitoxide")))]
    {
        command_git_status_file()
    }
}

/// Command exec git current tag
fn command_current_tag() -> Option<String> {
    Command::new("git")
        .args(["tag", "-l", "--contains", "HEAD"])
        .output()
        .map(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.map(|x| x.trim().to_string()))
        .unwrap_or(None)
}

/// git clean:git status --porcelain
/// check repository git status is clean
fn command_git_clean() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.map(|x| x.trim().to_string()))
        .map(|x| x.is_none() || x.map(|y| y.is_empty()).unwrap_or_default())
        .unwrap_or(true)
}

/// check git repository 'dirty or stage' status files.
/// git dirty:git status  --porcelain | grep '^\sM.' |awk '{print $2}'
/// git stage:git status --porcelain --untracked-files=all | grep '^[A|M|D|R]'|awk '{print $2}'
fn command_git_status_file() -> String {
    let git_status_files =
        move |args: &[&str], grep: &[&str], awk: &[&str]| -> SdResult<Vec<String>> {
            let git_shell = Command::new("git")
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            let git_out = git_shell.stdout.ok_or("Failed to exec git stdout")?;

            let grep_shell = Command::new("grep")
                .args(grep)
                .stdin(Stdio::from(git_out))
                .stdout(Stdio::piped())
                .spawn()?;
            let grep_out = grep_shell.stdout.ok_or("Failed to exec grep stdout")?;

            let mut awk_shell = Command::new("awk")
                .args(awk)
                .stdin(Stdio::from(grep_out))
                .stdout(Stdio::piped())
                .spawn()?;
            let mut awk_out = BufReader::new(
                awk_shell
                    .stdout
                    .as_mut()
                    .ok_or("Failed to exec awk stdout")?,
            );
            let mut line = String::new();
            awk_out.read_to_string(&mut line)?;
            Ok(line.lines().map(|x| x.into()).collect())
        };

    let dirty = git_status_files(&["status", "--porcelain"], &[r#"^\sM."#], &["{print $2}"])
        .unwrap_or_default();

    let stage = git_status_files(
        &["status", "--porcelain", "--untracked-files=all"],
        &[r#"^[A|M|D|R]"#],
        &["{print $2}"],
    )
    .unwrap_or_default();
    filter_git_dirty_stage(dirty, stage)
}

/// Command exec git current branch
fn command_current_branch() -> Option<String> {
    Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .map(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.map(|x| x.trim().to_string()))
        .unwrap_or(None)
}

fn filter_git_dirty_stage(dirty_files: Vec<String>, staged_files: Vec<String>) -> String {
    let mut concat_file = String::new();
    for file in dirty_files {
        concat_file.push_str("  * ");
        concat_file.push_str(&file);
        concat_file.push_str(" (dirty)\n");
    }
    for file in staged_files {
        concat_file.push_str("  * ");
        concat_file.push_str(&file);
        concat_file.push_str(" (staged)\n");
    }
    concat_file
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_std_env;
    use std::path::Path;

    #[test]
    fn test_git() {
        let env_map = get_std_env();
        let map = new_git(Path::new("./"), CiType::Github, &env_map);
        for (k, v) in map {
            assert!(!v.desc.is_empty());
            if !k.eq(TAG) && !k.eq(BRANCH) && !k.eq(GIT_STATUS_FILE) {
                assert!(!v.v.is_empty());
                continue;
            }

            //assert github tag always exist value
            if let Some(github_ref) = env_map.get("GITHUB_REF") {
                if github_ref.starts_with("refs/tags/") && k.eq(TAG) {
                    assert!(!v.v.is_empty(), "not empty");
                } else if github_ref.starts_with("refs/heads/") && k.eq(BRANCH) {
                    assert!(!v.v.is_empty());
                }
            }
        }
    }

    #[test]
    fn test_current_branch() {
        if get_std_env().get("GITHUB_REF").is_some() {
            return;
        }
        #[cfg(all(feature = "git2", not(feature = "gitoxide")))]
        {
            use crate::git::git2_mod::{git2_current_branch, git_repo};
            let git2_branch = git_repo(".")
                .map(|x| git2_current_branch(&x))
                .unwrap_or(None);
            let command_branch = command_current_branch();
            assert!(git2_branch.is_some());
            assert!(command_branch.is_some());
            assert_eq!(command_branch, git2_branch);
        }

        assert_eq!(Some(branch()), command_current_branch());
    }

    #[test]
    fn test_current_tag() {
        assert_eq!(Some(tag()), command_current_tag());
    }

    #[cfg(all(feature = "git2", feature = "gitoxide"))]
    #[test]
    fn compare_git2_vs_gitoxide_outputs() {
        let repo_path = Path::new(".");

        // Note: `init_git2` sometimes falls back onto `git` commands so we need to change the
        // cwd if `repo_path` is not `.`
        std::env::set_current_dir(repo_path).unwrap();

        let mut git2_map = new_empty_git(CiType::Github);
        git2_map.init_git2(repo_path).unwrap();

        let mut gitoxide_map = new_empty_git(CiType::Github);
        gitoxide_map.init_gitoxide(repo_path).unwrap();

        // only BRANCH and TAG will be overwritten by git
        let mut git_map = new_empty_git(CiType::Github);
        git_map.init(repo_path, &BTreeMap::new()).unwrap();

        git2_map
            .map
            .into_iter()
            // TODO: remove this filter once gitoxide implements `git status` like functionality
            .filter(|(key, _)| key.ne(&"GIT_CLEAN") && key.ne(&"GIT_STATUS_FILE"))
            .for_each(|(key, git2_val)| {
                let gix_val = gitoxide_map.map.get(key).unwrap();
                let git_val = git_map.map.get(key).unwrap();
                println!(
                    "{}\n  git2 - {:?}\n   gix - {:?}\n   git - {:?}\n",
                    key, git2_val.v, gix_val.v, git_val.v
                );

                // NOTES:
                // skipping assertion on `SHORT_COMMIT` as the current implementation just truncates the commit_id
                // to 8 characters, while gitoxide computes the shortest possible id while respecting the `core.abbrev` setting
                //
                // skipping assertions on `BRANCH` and `TAG` as the git2 values are always overwritten by the git value,
                // so we compare against that to make sure gix gives the correct output
                if !["SHORT_COMMIT", "BRANCH", "TAG"].contains(&key) {
                    assert_eq!(git2_val.v, gix_val.v, "mismatch found in {}", key);
                }

                if ["BRANCH", "TAG"].contains(&key) {
                    assert_eq!(git_val.v, gix_val.v, "mismatch found in {}", key);
                }
            });
    }
}
