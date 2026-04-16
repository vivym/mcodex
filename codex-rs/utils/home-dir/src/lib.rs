use codex_product_identity::MCODEX;
use codex_utils_absolute_path::AbsolutePathBuf;
use dirs::home_dir;
use std::io;
use std::path::PathBuf;

/// Returns the path to the Codex configuration directory, which can be
/// specified by the `MCODEX_HOME` environment variable. If not set, defaults to
/// `~/.mcodex`.
///
/// - If `MCODEX_HOME` is set, the value must exist and be a directory. The
///   value will be canonicalized and this function will `Err` otherwise.
/// - If `MCODEX_HOME` is not set, this function does not verify that the
///   directory exists.
pub fn find_codex_home() -> io::Result<AbsolutePathBuf> {
    let codex_home_env = std::env::var(MCODEX.home_env_var)
        .ok()
        .filter(|val| !val.is_empty());
    find_codex_home_from_env(codex_home_env.as_deref())
}

fn find_codex_home_from_env(active_home_env: Option<&str>) -> io::Result<AbsolutePathBuf> {
    match active_home_env.filter(|val| !val.is_empty()) {
        Some(val) => find_existing_home_dir(val, MCODEX.home_env_var),
        None => find_default_home_dir(MCODEX.default_home_dir_name),
    }
}

pub fn find_legacy_codex_home_for_migration(
    legacy_home_env: Option<&str>,
) -> io::Result<Option<AbsolutePathBuf>> {
    if let Some(val) = legacy_home_env.filter(|val| !val.is_empty()) {
        return find_existing_home_dir(val, MCODEX.legacy_home_env_var).map(Some);
    }

    let mut path = match home_dir() {
        Some(path) => path,
        None => return Ok(None),
    };
    path.push(MCODEX.legacy_home_dir_name);

    match find_existing_home_dir_from_path(path.clone()) {
        Ok(path) => Ok(Some(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("failed to read legacy Codex home {path:?}: {err}"),
        )),
    }
}

fn find_default_home_dir(home_dir_name: &str) -> io::Result<AbsolutePathBuf> {
    let mut path = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Could not find home directory"))?;
    path.push(home_dir_name);
    AbsolutePathBuf::from_absolute_path(path)
}

fn find_existing_home_dir(path: &str, env_var_name: &str) -> io::Result<AbsolutePathBuf> {
    find_existing_home_dir_from_path(PathBuf::from(path)).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => io::Error::new(
            io::ErrorKind::NotFound,
            format!("{env_var_name} points to {path:?}, but that path does not exist"),
        ),
        io::ErrorKind::InvalidInput => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{env_var_name} points to {path:?}, but that path is not a directory"),
        ),
        _ => io::Error::new(
            err.kind(),
            format!("failed to read {env_var_name} {path:?}: {err}"),
        ),
    })
}

fn find_existing_home_dir_from_path(path: PathBuf) -> io::Result<AbsolutePathBuf> {
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a directory",
        ));
    }

    let canonical = path.canonicalize()?;
    AbsolutePathBuf::from_absolute_path(canonical)
}

#[cfg(test)]
mod tests {
    use super::find_codex_home_from_env;
    use super::find_legacy_codex_home_for_migration;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use dirs::home_dir;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::Path;
    use tempfile::TempDir;

    fn expected_absolute(path: &Path) -> AbsolutePathBuf {
        let path = path.canonicalize().expect("canonicalize expected path");
        AbsolutePathBuf::from_absolute_path(path).expect("absolute home")
    }

    #[test]
    fn find_codex_home_env_missing_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-codex-home");
        let missing_str = missing
            .to_str()
            .expect("missing codex home path should be valid utf-8");

        let err = find_codex_home_from_env(Some(missing_str)).expect_err("missing MCODEX_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.to_string().contains("MCODEX_HOME"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_env_file_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let file_path = temp_home.path().join("codex-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let file_str = file_path
            .to_str()
            .expect("file codex home path should be valid utf-8");

        let err = find_codex_home_from_env(Some(file_str)).expect_err("file MCODEX_HOME");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_prefers_mcodex_home_env() {
        let temp_home = TempDir::new().expect("temp home");
        let resolved = find_codex_home_from_env(Some(
            temp_home
                .path()
                .to_str()
                .expect("temp codex home path should be valid utf-8"),
        ))
        .expect("resolve active home");

        assert_eq!(resolved, expected_absolute(temp_home.path()));
    }

    #[test]
    fn find_codex_home_without_env_uses_dot_mcodex() {
        let resolved = find_codex_home_from_env(None).expect("default home");

        assert!(resolved.as_path().ends_with(".mcodex"));
    }

    #[test]
    fn find_legacy_codex_home_for_migration_prefers_codex_home_env() {
        let legacy_home = TempDir::new().expect("legacy home");
        let resolved = find_legacy_codex_home_for_migration(Some(
            legacy_home
                .path()
                .to_str()
                .expect("legacy codex home path should be valid utf-8"),
        ))
        .expect("legacy home");

        assert_eq!(resolved, Some(expected_absolute(legacy_home.path())));
    }

    #[test]
    fn find_legacy_codex_home_for_migration_missing_codex_home_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-codex-home");
        let missing_str = missing
            .to_str()
            .expect("missing codex home path should be valid utf-8");

        let err = find_legacy_codex_home_for_migration(Some(missing_str))
            .expect_err("missing CODEX_HOME should fail");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.to_string().contains("CODEX_HOME"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_legacy_codex_home_for_migration_file_codex_home_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let file_path = temp_home.path().join("codex-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let file_str = file_path
            .to_str()
            .expect("file codex home path should be valid utf-8");

        let err = find_legacy_codex_home_for_migration(Some(file_str))
            .expect_err("file CODEX_HOME should fail");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("CODEX_HOME"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_ignores_codex_home_when_mcodex_home_is_unset() {
        let legacy_home = TempDir::new().expect("legacy home");
        let resolved = find_codex_home_from_env(None).expect("default home");

        assert_ne!(resolved, expected_absolute(legacy_home.path()));
        assert!(resolved.as_path().ends_with(".mcodex"));
    }

    #[test]
    fn find_codex_home_without_env_matches_home_dir() {
        let resolved = find_codex_home_from_env(None).expect("default home");
        let mut expected = home_dir().expect("home dir");
        expected.push(".mcodex");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }
}
