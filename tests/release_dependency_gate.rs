//! Fail-closed release checks for the four evgl-libs Git dependencies.
 //! Pure manifest/lockfile checks always run. On a protected main-branch
 //! release, CI additionally requires the pinned commit to exist on
 //! evgl-libs/main; PRs may test against reviewed-but-unmerged stack heads.

use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const LIB_URL: &str = "https://github.com/evento-globolo/evgl-libs";
const LIB_PACKAGES: [&str; 4] = [
    "evgl-domain",
    "evgl-provider-sdk",
    "evgl-providers",
    "evgl-token-vault",
];

fn manifest() -> &'static str {
    include_str!("../Cargo.toml")
}

fn lockfile() -> &'static str {
    include_str!("../Cargo.lock")
}

fn quoted_value<'a>(text: &'a str, field: &str) -> Result<&'a str, String> {
    let (_, rest) = text
        .split_once(field)
        .ok_or_else(|| format!("missing expected field {field:?}"))?;
    let (value, _) = rest
        .split_once('"')
        .ok_or_else(|| format!("unterminated quoted field {field:?}"))?;
    Ok(value)
}

fn is_exact_git_sha(pin: &str) -> bool {
    pin.len() == 40 && pin.bytes().all(|ch| ch.is_ascii_hexdigit())
}

fn verify_library_git_pins(manifest: &str, lockfile: &str) -> Result<String, String> {
    let mut expected_pin: Option<String> = None;

    for package in LIB_PACKAGES {
        let prefix = format!("{package} = ");
        let matching: Vec<_> = manifest
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(&prefix))
            .collect();
        if matching.len() != 1 {
            return Err(format!(
                "{package}: expected one manifest dependency, found {}",
                matching.len()
            ));
        }

        let line = matching[0];
        let expected_source = format!("git = \"{LIB_URL}\"");
        if !line.contains(&expected_source) {
            return Err(format!("{package}: unexpected Git repository"));
        }
        let pin = quoted_value(line, "rev = \"")?;
        if !is_exact_git_sha(pin) {
            return Err(format!("{package}: rev must be a full 40-character Git SHA"));
        }
        if let Some(existing) = &expected_pin {
            if existing != pin {
                return Err(format!("{package}: mixed evgl-libs Git revisions"));
            }
        } else {
            expected_pin = Some(pin.to_owned());
        }

        let expected_package_name = format!("name = \"{package}\"");
        let packages: Vec<_> = lockfile
            .split("[[package]]")
            .filter(|block| {
                block
                    .lines()
                    .any(|line| line.trim() == expected_package_name)
            })
            .collect();
        if packages.len() != 1 {
            return Err(format!(
                "{package}: expected one lockfile package, found {}",
                packages.len()
            ));
        }

        let block = packages[0];
        let source_fields: Vec<_> = block
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("source = "))
            .collect();
        let exact_source = format!("source = \"git+{LIB_URL}?rev={pin}#{pin}\"");
        if source_fields.len() != 1 || source_fields[0] != exact_source {
            return Err(format!(
                "{package}: lockfile Git source does not match the manifest pin"
            ));
        }
    }

    let pin = expected_pin.ok_or("no evgl-libs pins found")?;
    let repo_source = format!("source = \"git+{LIB_URL}?rev=");
    if lockfile.matches(&repo_source).count() != LIB_PACKAGES.len() {
        return Err("unexpected additional or missing evgl-libs lockfile entries".into());
    }

    Ok(pin)
}

#[test]
fn four_library_git_pins_and_lockfile_sources_match() {
    let pin = verify_library_git_pins(manifest(), lockfile())
        .expect("all four lib crates must share the exact same commit and lockfile source");
    assert_eq!(pin.len(), 40);
}

#[test]
fn manifest_revision_mismatch_fails_closed() {
    let source = manifest().replacen(
        "evgl-domain = { git = \"https://github.com/evento-globolo/evgl-libs\", rev = \"",
        "evgl-domain = { git = \"https://github.com/evento-globolo/evgl-libs\", rev = \"0000000000000000000000000000000000000000",
        1,
    );
    assert!(verify_library_git_pins(&source, lockfile()).is_err());
}

#[test]
fn branch_alias_or_truncated_revision_fails_closed() {
    let source = manifest().replacen("rev = \"", "rev = \"main-", 1);
    assert!(verify_library_git_pins(&source, lockfile()).is_err());
}

#[test]
fn missing_dependency_and_duplicate_package_fail_closed() {
    let source = manifest().replacen("evgl-domain = ", "hidden-evgl-domain = ", 1);
    assert!(verify_library_git_pins(&source, lockfile()).is_err());

    let duplicate = format!(
        "{}\n[[package]]\nname = \"evgl-domain\"\nsource = \"fake\"\n",
        lockfile()
    );
    assert!(verify_library_git_pins(manifest(), &duplicate).is_err());
}

#[test]
fn mismatched_lockfile_git_source_fails_closed() {
    let pin = verify_library_git_pins(manifest(), lockfile()).unwrap();
    let lock_source = format!("git+{LIB_URL}?rev={pin}#{pin}");
    let inconsistent = lockfile().replacen(
        &lock_source,
        "git+https://github.com/evento-globolo/evgl-libs?rev=0000000000000000000000000000000000000000#0000000000000000000000000000000000000000",
        1,
    );
    assert!(verify_library_git_pins(manifest(), &inconsistent).is_err());
}

#[test]
fn unexpected_git_repo_or_extra_package_fails_closed() {
    let wrong_host = manifest().replacen(
        LIB_URL,
        "https://github.com/untrusted-fork/evgl-libs",
        1,
    );
    assert!(verify_library_git_pins(&wrong_host, lockfile()).is_err());

    let pin = verify_library_git_pins(manifest(), lockfile()).unwrap();
    let extra = format!(
        "{}\n[[package]]\nname = \"rogue-source\"\nsource = \"git+{LIB_URL}?rev={pin}#{pin}\"\n",
        lockfile()
    );
    assert!(verify_library_git_pins(manifest(), &extra).is_err());
}

struct TemporaryRepository(PathBuf);

impl Drop for TemporaryRepository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn protected_main_requires_git_pin_reachable_from_library_main() {
    if std::env::var("EVGL_REQUIRE_LIB_MAIN_ANCESTRY").as_deref() != Ok("true") {
        return;
    }

    let pin = verify_library_git_pins(manifest(), lockfile())
        .expect("manifest and lockfile must match before contacting remote");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be valid")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "evgl-release-lib-ancestry-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create isolated temporary Git directory");
    let _cleanup = TemporaryRepository(directory.clone());

    let initialized = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("Git is required for protected release checking");
    assert!(initialized.success(), "unable to create release verification Git repository");

    let fetched = Command::new("git")
        .arg("-C")
        .arg(&directory)
        .args(["fetch", "--quiet", "--no-tags", "--filter=blob:none"])
        .arg(format!("{LIB_URL}.git"))
        .arg("main")
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("fetch evgl-libs/main for protected release gate");
    assert!(
        fetched.success(),
        "cannot verify evgl-libs/main; aborting protected release"
    );

    let check = Command::new("git")
        .arg("-C")
        .arg(&directory)
        .args(["merge-base", "--is-ancestor", &pin, "FETCH_HEAD"])
        .status()
        .expect("verify pinned library commit ancestry");
    assert!(
        check.success(),
        "the pinned library commit is not reachable from evgl-libs/main; release must remain blocked"
    );
}
