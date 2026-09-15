use std::path::Path;

use sdkwork_utils_rust::sha256_hash;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const PROVIDER_SESSION_DIRECTORY_FINGERPRINT_PREFIX: &str =
    "sdkwork.provider-session-directory.v1\n";

pub fn normalize_provider_session_path(value: &str) -> String {
    let mut normalized = value.trim().replace('\\', "/");
    if let Some(path) = normalized.strip_prefix("//?/") {
        normalized = path.to_string();
    }
    while normalized.len() > 1 && normalized.ends_with('/') {
        if normalized.len() == 3 && normalized.as_bytes()[1] == b':' {
            break;
        }
        normalized.pop();
    }
    if normalized.as_bytes().get(1) == Some(&b':') {
        normalized.make_ascii_lowercase();
    }
    normalized
}

pub fn provider_session_path_basename(value: &str) -> Option<String> {
    normalize_provider_session_path(value)
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| segment.to_ascii_lowercase())
}

pub fn provider_session_directory_fingerprint(value: &str) -> std::io::Result<String> {
    let path = Path::new(value.trim());
    let mut entries = std::fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let kind = if file_type.is_dir() {
                'd'
            } else if file_type.is_file() {
                'f'
            } else {
                'o'
            };
            Ok((entry.file_name().to_string_lossy().into_owned(), kind))
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

    let mut manifest = String::from(PROVIDER_SESSION_DIRECTORY_FINGERPRINT_PREFIX);
    for (name, kind) in entries {
        manifest.push(kind);
        manifest.push('\0');
        manifest.push_str(&name);
        manifest.push('\n');
    }
    Ok(format!("sha256:{}", sha256_hash(manifest.as_bytes())))
}

pub fn epoch_millis_to_rfc3339(epoch_millis: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(epoch_millis) * 1_000_000)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_extended_paths_and_separators() {
        // WORKSPACE-PATH:allow — the absolute paths are the *subject* of the
        // assertion (this test pins how the normalizer lowercases a drive-prefixed
        // workspace path and strips the `\\?\` extended-length prefix), not a
        // source or build binding.
        assert_eq!(
            normalize_provider_session_path(r"\\?\E:\SDKWork-Space\BirdCoder\"),
            "e:/sdkwork-space/birdcoder"
        );
        // WORKSPACE-PATH:allow — expected-output string of the assertion above;
        // it must stay literal for the comparison to mean anything.
        assert_eq!(
            normalize_provider_session_path("E:/sdkwork-space/birdcoder"),
            "e:/sdkwork-space/birdcoder"
        );
    }

    #[test]
    fn formats_epoch_millis_as_rfc3339() {
        assert_eq!(
            epoch_millis_to_rfc3339(0).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
    }

    #[test]
    fn fingerprints_directory_entry_names_and_kinds_deterministically() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "sdkwork-provider-session-directory-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src")).expect("create fixture directory");
        std::fs::write(root.join("README.md"), "fixture").expect("create fixture file");

        let first = provider_session_directory_fingerprint(root.to_str().expect("fixture path"))
            .expect("fingerprint fixture");
        let second = provider_session_directory_fingerprint(root.to_str().expect("fixture path"))
            .expect("fingerprint fixture again");
        assert_eq!(first, second);
        assert_eq!(
            first,
            "sha256:501fa61985d3b2c255fdb3816cfa1f20953812554fbeb8dd07c2b18b89388913"
        );

        std::fs::write(root.join("package.json"), "{}").expect("create second fixture file");
        let changed = provider_session_directory_fingerprint(root.to_str().expect("fixture path"))
            .expect("fingerprint changed fixture");
        assert_ne!(first, changed);

        std::fs::remove_dir_all(root).expect("remove fixture directory");
    }
}
