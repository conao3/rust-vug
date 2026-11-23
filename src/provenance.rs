use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::IpAddr;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ureq::Agent;
use ureq::config::Config;
use ureq::http::{Uri, header};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::Url;

use crate::merge::{CheckResult, Identity, Status, is_hex, model};
use crate::naming;
use vug::fetch::sha256_reader;

const RECORD_LIMIT: u64 = 1024 * 1024;
const PATCH_LIMIT: u64 = 8 * 1024 * 1024;
const SOURCE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    identity: Identity,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u32,
    nixpkgs_commit: String,
    new_package_sources: BTreeMap<String, Vec<String>>,
    approved_sources: BTreeMap<String, Vec<String>>,
    redirect_sources: BTreeMap<String, Vec<String>>,
    patch_origins: BTreeMap<String, PatchOrigin>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PatchOrigin {
    Nixpkgs {
        path: String,
    },
    Upstream {
        repository: String,
        raw_root: String,
        commit: String,
        path: String,
    },
}

#[derive(Deserialize)]
struct Package {
    upstream: Upstream,
}

#[derive(Deserialize)]
struct Upstream {
    sources: Vec<String>,
}

#[derive(Deserialize)]
struct Version {
    source: Source,
    #[serde(default)]
    patches: Vec<String>,
}

#[derive(Deserialize)]
struct Source {
    url: String,
    sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Report {
    schema_version: u32,
    results: Vec<CheckResult>,
    findings: Vec<Finding>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Finding {
    check: u8,
    path: String,
    code: &'static str,
}

impl Report {
    pub(crate) fn summary(&self, separator: &str) -> String {
        self.results
            .iter()
            .map(|r| {
                let status = match r.status {
                    Status::Passed => "passed",
                    Status::Failed => "failed",
                    Status::Incomplete => "incomplete",
                    Status::RequiresApproval => "requires_approval",
                };
                format!("{}{separator}{}{separator}{status}\n", r.check, r.target)
            })
            .collect()
    }
}

#[derive(Default)]
struct Changes {
    package_record: bool,
    versions: BTreeSet<String>,
    patches: BTreeSet<String>,
}

struct Git<'a>(&'a Path);

impl Git<'_> {
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let result = Command::new("git")
            .arg("--no-replace-objects")
            .arg("--literal-pathspecs")
            .args(["-C"])
            .arg(self.0)
            .args(args)
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !result.status.success() {
            return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
        }
        Ok(result.stdout)
    }

    fn changes(&self, identity: &Identity) -> Result<BTreeMap<String, Changes>, String> {
        for oid in [&identity.base_sha, &identity.head_sha] {
            if self.run(&["cat-file", "-t", oid])? != b"commit\n" {
                return Err("base_sha and head_sha must identify Git commits".into());
            }
        }
        let tree_type = self.run(&["cat-file", "-t", &identity.tree_oid])?;
        if tree_type != b"tree\n" {
            return Err("tree_oid must identify a Git tree".into());
        }
        let diff = self.run(&[
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-renames",
            &identity.base_sha,
            &identity.tree_oid,
            "--",
            "packages/",
        ])?;
        let mut packages = BTreeMap::<String, Changes>::new();
        for path in git_lines(&diff)? {
            let parts: Vec<_> = path.split('/').collect();
            let ["packages", shard, package, rest @ ..] = parts.as_slice() else {
                continue;
            };
            if rest.is_empty()
                || !naming::model::valid_name(package)
                || naming::model::shard(package) != *shard
            {
                continue;
            }
            let changes = packages.entry((*package).into()).or_default();
            match rest {
                ["package.toml"] => changes.package_record = true,
                ["versions", file]
                    if file
                        .strip_suffix(".toml")
                        .is_some_and(naming::model::valid_version) =>
                {
                    changes.versions.insert(path.into());
                }
                ["patches", file] if file.ends_with(".patch") => {
                    changes.patches.insert(path.into());
                }
                _ => {}
            }
        }
        Ok(packages)
    }

    fn blobs(&self, tree: &str, path: &str) -> Result<BTreeMap<String, String>, String> {
        let output = self.run(&["ls-tree", "-r", "-z", tree, "--", path])?;
        git_lines(&output)?
            .map(|entry| {
                let (info, path) = entry.split_once('\t').ok_or("invalid Git tree entry")?;
                let (mode, oid) = info.split_once(" blob ").unwrap_or_default();
                let oid = if matches!(mode, "100644" | "100755") {
                    oid
                } else {
                    ""
                };
                Ok((path.into(), oid.into()))
            })
            .collect()
    }

    fn blob(&self, oid: &str, limit: u64) -> Result<Vec<u8>, String> {
        if oid.is_empty() {
            return Err("expected a regular Git blob".into());
        }
        let size = self.run(&["cat-file", "-s", oid])?;
        let size: u64 = String::from_utf8_lossy(&size)
            .trim()
            .parse()
            .map_err(|_| "invalid Git object size")?;
        if size > limit {
            return Err(format!("Git blob exceeds the {limit}-byte limit"));
        }
        self.run(&["cat-file", "blob", oid])
    }

    fn record<T: for<'de> Deserialize<'de>>(&self, oid: &str, path: &str) -> Result<T, String> {
        let content = self
            .blob(oid, RECORD_LIMIT)
            .map_err(|e| format!("{path}: {e}"))?;
        let text = std::str::from_utf8(&content).map_err(|e| format!("{path}: {e}"))?;
        toml::from_str(text).map_err(|e| format!("{path}: {e}"))
    }
}

fn git_lines(bytes: &[u8]) -> Result<impl Iterator<Item = &str>, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("Git paths must be UTF-8: {e}"))?;
    Ok(text.split_terminator('\0'))
}

fn parse_url(value: &str) -> Result<Url, &'static str> {
    if value
        .chars()
        .any(|ch| ch.is_control() || ch == '\\' || ch == ' ')
        || !value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        return Err("expected an HTTPS URL");
    }
    let authority = value[8..].split(['/', '?', '#']).next().unwrap_or_default();
    let url = Url::parse(value).map_err(|_| "invalid URL")?;
    if authority.is_empty()
        || authority.contains('@')
        || url.fragment().is_some()
        || url.port_or_known_default() == Some(0)
    {
        return Err("URL contains credentials, a fragment or an invalid authority");
    }
    let path = url.path().to_ascii_lowercase();
    if [
        "//", ";", "%2f", "%5c", "%2e", "%25", "%3f", "%23", "%3b", "%0", "%1", "%7f",
    ]
    .iter()
    .any(|escape| path.contains(escape))
        || path.split('%').skip(1).any(|part| {
            !part
                .as_bytes()
                .get(..2)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        })
    {
        return Err("URL contains an ambiguous path escape");
    }
    Ok(url)
}

fn parse_origin(value: &str) -> Result<Url, String> {
    let url = parse_url(value).map_err(|e| format!("invalid origin: {e}"))?;
    if !url.path().ends_with('/') || url.query().is_some() {
        return Err("origin paths must end in a slash and have no query".into());
    }
    Ok(url)
}

fn parse_origins(values: &[String]) -> Result<Vec<Url>, String> {
    values.iter().map(|value| parse_origin(value)).collect()
}

fn matches_origin(origin: &Url, url: &Url) -> bool {
    model::origin_matches(
        origin.host_str().expect("HTTPS host"),
        origin.port_or_known_default().expect("HTTPS port").into(),
        origin.path(),
        url.host_str().expect("HTTPS host"),
        url.port_or_known_default().expect("HTTPS port").into(),
        url.path(),
    )
}

fn safe_segment(segment: &str) -> bool {
    !matches!(segment, "" | "." | "..")
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._+".contains(&b))
}

impl Policy {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || !is_hex(&self.nixpkgs_commit, 40) {
            return Err("expected policy schema_version 1 and a full nixpkgs commit".into());
        }
        for (package, origins) in self
            .new_package_sources
            .iter()
            .chain(&self.approved_sources)
            .chain(&self.redirect_sources)
        {
            if !naming::model::valid_name(package) || origins.is_empty() {
                return Err(format!("invalid origins for package {package}"));
            }
            parse_origins(origins)?;
        }
        for (candidate, origin) in &self.patch_origins {
            let parts: Vec<_> = candidate.split('/').collect();
            if !matches!(parts.as_slice(), ["packages", shard, package, "patches", file]
                if naming::model::valid_name(package)
                    && naming::model::shard(package) == *shard
                    && file.ends_with(".patch"))
            {
                return Err(format!("{candidate}: expected a candidate patch path"));
            }
            origin.url(&self.nixpkgs_commit)?;
        }
        Ok(())
    }
}

impl PatchOrigin {
    fn url(&self, nixpkgs_commit: &str) -> Result<Url, String> {
        let (raw_root, commit, path) = match self {
            Self::Nixpkgs { path } => (
                "https://raw.githubusercontent.com/NixOS/nixpkgs/",
                nixpkgs_commit,
                path,
            ),
            Self::Upstream {
                repository,
                raw_root,
                commit,
                path,
            } => {
                parse_origin(repository)?;
                if !is_hex(commit, 40) {
                    return Err("expected a full upstream Git commit".into());
                }
                (raw_root.as_str(), commit.as_str(), path)
            }
        };
        if !path.split('/').all(safe_segment) {
            return Err("expected a repository-relative patch origin path".into());
        }
        let raw_root = parse_origin(raw_root)?;
        parse_url(&format!("{raw_root}{commit}/{path}")).map_err(str::to_owned)
    }

    fn is_allowed(&self, origins: &[Url]) -> bool {
        let Self::Upstream { repository, .. } = self else {
            return true;
        };
        let repository = parse_origin(repository).expect("validated upstream repository");
        origins
            .iter()
            .any(|origin| matches_origin(&repository, origin))
    }
}

#[derive(Debug, Default)]
struct PublicResolver(DefaultResolver);

impl Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let mut result = self.empty();
        for address in &self.0.resolve(uri, config, timeout)? {
            if public_address(address.ip()) {
                result.push(*address);
            }
        }
        if result.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(result)
    }
}

fn public_address(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => !matches!(
            ip.octets(),
            [0 | 10 | 127 | 224..=255, ..]
                | [100, 64..=127, ..]
                | [169, 254, ..]
                | [172, 16..=31, ..]
                | [192, 0, 0 | 2, _]
                | [192, 88, 99, _]
                | [192, 168, ..]
                | [198, 18 | 19, ..]
                | [198, 51, 100, _]
                | [203, 0, 113, _]
        ),
        IpAddr::V6(ip) => {
            matches!(ip.segments()[0], 0x2000..=0x3fff)
                && !matches!(
                    ip.segments(),
                    [0x2001, 0..=0x01ff | 0x0db8, ..] | [0x2002 | 0x3fff, ..]
                )
        }
    }
}

#[derive(Clone, Copy)]
struct Issue {
    status: Status,
    code: &'static str,
}

impl Issue {
    fn failed(code: &'static str) -> Self {
        Self {
            status: Status::Failed,
            code,
        }
    }

    fn incomplete(code: &'static str) -> Self {
        Self {
            status: Status::Incomplete,
            code,
        }
    }

    fn approval(code: &'static str) -> Self {
        Self {
            status: Status::RequiresApproval,
            code,
        }
    }
}

trait Downloads {
    fn open(
        &self,
        url: Url,
        redirect_allowed: &dyn Fn(&Url) -> bool,
        limit: u64,
    ) -> Result<Box<dyn Read>, Issue>;
}

impl Downloads for Agent {
    fn open(
        &self,
        mut url: Url,
        redirect_allowed: &dyn Fn(&Url) -> bool,
        limit: u64,
    ) -> Result<Box<dyn Read>, Issue> {
        for _ in 0..=5 {
            let response = self
                .get(url.as_str())
                .header(header::ACCEPT_ENCODING, "identity")
                .call()
                .map_err(|_| Issue::incomplete("download_unavailable"))?;
            match response.status().as_u16() {
                200 if response
                    .headers()
                    .get(header::CONTENT_ENCODING)
                    .is_none_or(|value| value == "identity") =>
                {
                    return Ok(Box::new(
                        response
                            .into_body()
                            .into_with_config()
                            .limit(limit)
                            .reader(),
                    ));
                }
                301 | 302 | 303 | 307 | 308 => {
                    let location = response
                        .headers()
                        .get(header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| Issue::incomplete("invalid_redirect"))?;
                    url = redirect_url(&url, location)
                        .filter(|next| redirect_allowed(next))
                        .ok_or_else(|| Issue::failed("redirect_outside_origins"))?;
                }
                _ => return Err(Issue::incomplete("download_unavailable")),
            }
        }
        Err(Issue::incomplete("redirect_limit"))
    }
}

fn redirect_url(url: &Url, location: &str) -> Option<Url> {
    if location
        .chars()
        .any(|ch| ch.is_control() || ch == '\\' || ch == ' ')
    {
        return None;
    }
    let joined = url.join(location).ok()?;
    parse_url(joined.as_str()).ok()
}

fn check_hash(
    downloads: &impl Downloads,
    url: Url,
    allowed: &[Url],
    redirects: &[Url],
    digest: &str,
) -> Result<(), Issue> {
    if !is_hex(digest, 64) {
        return Err(Issue::failed("invalid_source_hash"));
    }
    let reader = downloads.open(
        url,
        &|next| {
            allowed
                .iter()
                .chain(redirects)
                .any(|origin| matches_origin(origin, next))
        },
        SOURCE_LIMIT,
    )?;
    let actual = sha256_reader(reader).map_err(|_| Issue::incomplete("source_read_failed"))?;
    if actual != digest {
        return Err(Issue::failed("source_hash_mismatch"));
    }
    Ok(())
}

fn check_patch(
    downloads: &impl Downloads,
    policy: &Policy,
    origins: &[Url],
    path: &str,
    candidate: &[u8],
) -> Result<(), Issue> {
    let origin = policy
        .patch_origins
        .get(path)
        .filter(|origin| origin.is_allowed(origins))
        .ok_or_else(|| Issue::approval("patch_origin_unapproved"))?;
    let url = origin
        .url(&policy.nixpkgs_commit)
        .expect("validated patch origin");
    let mut reader = downloads.open(url.clone(), &|next| *next == url, PATCH_LIMIT)?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|_| Issue::incomplete("patch_read_failed"))?;
    if candidate != bytes {
        return Err(Issue::failed("patch_bytes_mismatch"));
    }
    Ok(())
}

fn note(
    result: Result<(), Issue>,
    status: &mut Status,
    check: u8,
    path: &str,
    findings: &mut Vec<Finding>,
) {
    if let Err(issue) = result {
        *status = status.combine(issue.status);
        findings.push(Finding {
            check,
            path: path.into(),
            code: issue.code,
        });
    }
}

struct Inspection<'a, D> {
    git: Git<'a>,
    identity: Identity,
    policy: Policy,
    downloads: D,
}

impl<D: Downloads> Inspection<'_, D> {
    fn package(
        &self,
        package: &str,
        changes: Changes,
        findings: &mut Vec<Finding>,
    ) -> Result<[Status; 3], String> {
        let directory = naming::model::package_dir(package);
        let package_path = format!("{directory}/package.toml");
        let base = self.git.blobs(&self.identity.base_sha, &package_path)?;
        let tree = self.git.blobs(&self.identity.tree_oid, &directory)?;
        let candidate = tree.get(&package_path);
        let mut origins = Vec::new();
        if let Some(oid) = base.get(&package_path) {
            let record: Package = self.git.record(oid, &package_path)?;
            origins = parse_origins(&record.upstream.sources)?;
        } else if let Some(sources) = self.policy.new_package_sources.get(package) {
            origins = parse_origins(sources)?;
        }
        if let Some(sources) = self.policy.approved_sources.get(package) {
            origins.extend(parse_origins(sources)?);
        }
        let candidate_origins = candidate
            .map(|oid| self.git.record::<Package>(oid, &package_path))
            .transpose()?
            .map(|record| parse_origins(&record.upstream.sources))
            .transpose()?
            .unwrap_or_default();
        let redirects = self
            .policy
            .redirect_sources
            .get(package)
            .map(|sources| parse_origins(sources))
            .transpose()?
            .unwrap_or_default();

        let mut referenced_patches = BTreeSet::new();
        let patch_prefix = format!("{directory}/patches/");
        let version_prefix = format!("{directory}/versions/");
        let mut sources = Vec::new();
        let mut status = [Status::Passed; 3];
        for (path, oid) in tree.range(version_prefix.clone()..) {
            let Some(file) = path.strip_prefix(&version_prefix) else {
                break;
            };
            if !file
                .strip_suffix(".toml")
                .is_some_and(naming::model::valid_version)
            {
                continue;
            }
            let directly_changed = changes.package_record || changes.versions.contains(path);
            if !directly_changed && changes.patches.is_empty() {
                continue;
            }
            let version: Version = self.git.record(oid, path)?;
            let version_patches = version
                .patches
                .iter()
                .map(|patch| {
                    if patch.contains('/') || !patch.ends_with(".patch") {
                        return Err(format!("{path}: invalid patch name"));
                    }
                    Ok(format!("{patch_prefix}{patch}"))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if !directly_changed
                && !version_patches
                    .iter()
                    .any(|patch| changes.patches.contains(patch))
            {
                continue;
            }
            referenced_patches.extend(
                version_patches
                    .into_iter()
                    .filter(|patch| directly_changed || changes.patches.contains(patch)),
            );
            let allowed = parse_url(&version.source.url)
                .map_err(|_| Issue::failed("invalid_source_url"))
                .and_then(|url| {
                    if !candidate_origins
                        .iter()
                        .any(|origin| matches_origin(origin, &url))
                    {
                        return Err(Issue::failed("source_outside_candidate_origins"));
                    }
                    if !origins.iter().any(|origin| matches_origin(origin, &url)) {
                        return Err(Issue::approval("source_origin_unapproved"));
                    }
                    sources.push((path, url, version.source.sha256));
                    Ok(())
                });
            note(allowed, &mut status[0], 3, path, findings);
        }
        if status[0] == Status::Passed {
            for (path, url, digest) in sources {
                note(
                    check_hash(&self.downloads, url, &origins, &redirects, &digest),
                    &mut status[1],
                    4,
                    path,
                    findings,
                );
            }
        } else {
            note(
                Err(Issue::incomplete("source_origin_pending")),
                &mut status[1],
                4,
                &package_path,
                findings,
            );
        }
        if status[1] == Status::Passed {
            for path in changes.patches.union(&referenced_patches) {
                let result = match tree.get(path) {
                    Some(oid) => match self.git.blob(oid, PATCH_LIMIT) {
                        Ok(bytes) => {
                            check_patch(&self.downloads, &self.policy, &origins, path, &bytes)
                        }
                        Err(_) => Err(Issue::failed("invalid_candidate_patch")),
                    },
                    None if !referenced_patches.contains(path) => continue,
                    None => Err(Issue::failed("candidate_patch_missing")),
                };
                note(result, &mut status[2], 5, path, findings);
            }
        } else {
            note(
                Err(Issue::incomplete("source_hash_pending")),
                &mut status[2],
                5,
                &package_path,
                findings,
            );
        }
        Ok(status)
    }
}

pub(crate) fn verify(root: &Path, input: &str, policy: &str) -> Result<Report, String> {
    let config = Agent::config_builder()
        .https_only(true)
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build();
    let downloads = Agent::with_parts(
        config,
        DefaultConnector::default(),
        PublicResolver::default(),
    );
    verify_with(root, input, policy, downloads)
}

fn verify_with(
    root: &Path,
    input: &str,
    policy: &str,
    downloads: impl Downloads,
) -> Result<Report, String> {
    let request: Request = serde_json::from_str(input).map_err(|e| e.to_string())?;
    if request.schema_version != 1 {
        return Err("expected schema_version 1".into());
    }
    request.identity.validate()?;
    if sha256_reader(policy.as_bytes()).map_err(|e| e.to_string())?
        != request.identity.policy_digest
    {
        return Err("policy bytes do not match policy_digest".into());
    }
    let policy: Policy = serde_json::from_str(policy).map_err(|e| e.to_string())?;
    policy.validate()?;
    let inspection = Inspection {
        git: Git(root),
        identity: request.identity,
        policy,
        downloads,
    };
    let mut report = Report {
        schema_version: 1,
        results: Vec::new(),
        findings: Vec::new(),
    };
    for (package, changes) in inspection.git.changes(&inspection.identity)? {
        for (check, status) in
            (3..=5).zip(inspection.package(&package, changes, &mut report.findings)?)
        {
            report.results.push(CheckResult {
                identity: inspection.identity.clone(),
                check,
                target: package.clone(),
                status,
                output_id: None,
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    const SOURCE: &str = "https://github.com/test/demo/archive/v1.tar.gz";
    const CANONICAL_SOURCE: &str = "https://codeload.github.com/test/demo/tar.gz/v1";
    const NIXPKGS_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const PATCH: &str = "packages/de/demo/patches/1-fix.patch";
    const UPSTREAM: &str = "https://github.com/test/demo/";

    #[derive(Default)]
    struct DownloadsFixture {
        files: BTreeMap<&'static str, &'static [u8]>,
        redirects: BTreeMap<&'static str, &'static str>,
    }

    impl Downloads for DownloadsFixture {
        fn open(
            &self,
            mut url: Url,
            allowed: &dyn Fn(&Url) -> bool,
            limit: u64,
        ) -> Result<Box<dyn Read>, Issue> {
            if let Some(next) = self.redirects.get(url.as_str()) {
                url = redirect_url(&url, next)
                    .filter(allowed)
                    .ok_or_else(|| Issue::failed("redirect_outside_origins"))?;
            }
            let bytes = self
                .files
                .get(url.as_str())
                .filter(|bytes| bytes.len() as u64 <= limit)
                .ok_or_else(|| Issue::incomplete("download_unavailable"))?;
            Ok(Box::new(Cursor::new(*bytes)))
        }
    }

    struct Fixture {
        root: TempDir,
        base: String,
    }

    impl Fixture {
        fn new(origin: Option<&str>) -> Self {
            let mut fixture = Self {
                root: tempfile::tempdir().unwrap(),
                base: String::new(),
            };
            fixture.git(&["init", "-q"]);
            fixture.git(&["config", "user.name", "Fixture"]);
            fixture.git(&["config", "user.email", "fixture@example.test"]);
            fixture.git(&["config", "core.hooksPath", "/dev/null"]);
            if let Some(origin) = origin {
                fixture.package_record(origin);
            }
            fixture.commit();
            fixture.base = fixture.git(&["rev-parse", "HEAD"]);
            fixture
        }

        fn git(&self, args: &[&str]) -> String {
            String::from_utf8(Git(self.root.path()).run(args).unwrap())
                .unwrap()
                .trim()
                .into()
        }

        fn write(&self, path: &str, content: &[u8]) {
            let path = self.root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn commit(&self) {
            self.git(&["add", "--all"]);
            self.git(&["commit", "-qm", "Fixture", "--allow-empty", "--no-gpg-sign"]);
        }

        fn package_record(&self, origin: &str) {
            self.write(
                "packages/de/demo/package.toml",
                format!("[upstream]\nsources = [\"{origin}\"]\n").as_bytes(),
            );
        }

        fn version(&self, url: &str, bytes: &[u8], patch: bool) {
            let patches = if patch {
                "patches = [\"1-fix.patch\"]\n"
            } else {
                ""
            };
            let hash = sha256_reader(bytes).unwrap();
            self.write(
                "packages/de/demo/versions/1.toml",
                format!("{patches}[source]\nurl = \"{url}\"\nsha256 = \"{hash}\"\n").as_bytes(),
            );
        }

        fn verify(&self, policy: &Value, downloads: DownloadsFixture) -> Report {
            let policy = policy.to_string();
            let identity = Identity {
                base_sha: self.base.clone(),
                head_sha: self.git(&["rev-parse", "HEAD"]),
                tree_oid: self.git(&["rev-parse", "HEAD^{tree}"]),
                policy_digest: sha256_reader(policy.as_bytes()).unwrap(),
            };
            let input = json!({"schema_version": 1, "identity": identity}).to_string();
            let report = verify_with(self.root.path(), &input, &policy, downloads).unwrap();
            let roundtrip = serde_json::to_value(&report).unwrap();
            let results =
                serde_json::from_value::<Vec<CheckResult>>(roundtrip["Results"].clone()).unwrap();
            assert!(
                results
                    .iter()
                    .all(|result| result.identity == identity && result.target == "demo")
            );
            report
        }
    }

    fn policy() -> Value {
        json!({
            "schema_version": 1,
            "nixpkgs_commit": NIXPKGS_COMMIT,
            "new_package_sources": {"demo": [UPSTREAM]},
            "approved_sources": {},
            "redirect_sources": {"demo": ["https://codeload.github.com/test/demo/"]},
            "patch_origins": {
                PATCH: {"kind": "nixpkgs", "path": "pkgs/demo/fix.patch"}
            }
        })
    }

    fn statuses(report: &Report) -> Vec<Status> {
        report.results.iter().map(|result| result.status).collect()
    }

    #[test]
    fn url_components_and_public_addresses_enforce_boundaries() {
        let origin = parse_url("https://GitHub.com:443/test/demo/").unwrap();
        assert!(matches_origin(&origin, &parse_url(SOURCE).unwrap()));
        for value in [
            "https://github.com:444/test/demo/file",
            "https://github.com.evil.test/test/demo/file",
            "https://github.com/test/demolition/file",
            "https://github.com/test/demo/../another/file",
        ] {
            assert!(
                !matches_origin(&origin, &parse_url(value).unwrap()),
                "{value}"
            );
        }
        for value in [
            "http://github.com/test/demo/file",
            "https://user@github.com/test/demo/file",
            "https://github.com/test/demo/file#fragment",
            "https://github.com/test/demo/%2f..%2fsecret",
            "https://github.com/test/demo/%252f..",
        ] {
            assert!(parse_url(value).is_err(), "{value}");
        }
        assert!(!matches_origin(
            &origin,
            &redirect_url(&origin, "//other.test/test/demo/file").unwrap()
        ));
        assert!(redirect_url(&origin, "http://github.com/test/demo/file").is_none());
        for ip in [
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "10.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_address(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn pinned_source_hash_and_patch_bytes_are_checked() {
        let fixture = Fixture::new(None);
        fixture.package_record(UPSTREAM);
        fixture.version(SOURCE, b"source bytes", true);
        fixture.write(PATCH, b"patch bytes");
        fixture.commit();
        fixture.write(PATCH, b"uncommitted patch bytes");
        let patch_url = "https://raw.githubusercontent.com/NixOS/nixpkgs/0123456789abcdef0123456789abcdef01234567/pkgs/demo/fix.patch";
        let downloads = |source, patch| DownloadsFixture {
            files: BTreeMap::from([(CANONICAL_SOURCE, source), (patch_url, patch)]),
            redirects: BTreeMap::from([(SOURCE, CANONICAL_SOURCE)]),
        };
        let policy = policy();
        let report = fixture.verify(&policy, downloads(b"source bytes", b"patch bytes"));
        assert_eq!(statuses(&report), [Status::Passed; 3]);
        let report = fixture.verify(&policy, downloads(b"other source", b"patch bytes"));
        assert_eq!(
            statuses(&report),
            [Status::Passed, Status::Failed, Status::Incomplete]
        );
        assert_eq!(report.findings[0].code, "source_hash_mismatch");
        let report = fixture.verify(&policy, downloads(b"source bytes", b"other patch"));
        assert_eq!(
            statuses(&report),
            [Status::Passed, Status::Passed, Status::Failed]
        );
        assert_eq!(report.findings[0].code, "patch_bytes_mismatch");
        let report = fixture.verify(&policy, DownloadsFixture::default());
        assert_eq!(
            statuses(&report),
            [Status::Passed, Status::Incomplete, Status::Incomplete]
        );
        assert_eq!(report.findings[0].code, "download_unavailable");
        let report = fixture.verify(
            &policy,
            DownloadsFixture {
                files: BTreeMap::from([(SOURCE, b"source bytes".as_slice())]),
                ..DownloadsFixture::default()
            },
        );
        assert_eq!(
            statuses(&report),
            [Status::Passed, Status::Passed, Status::Incomplete]
        );
        assert_eq!(report.findings[0].path, PATCH);
        assert_eq!(report.findings[0].code, "download_unavailable");

        let mut unpinned = policy.clone();
        unpinned["patch_origins"] = json!({});
        let report = fixture.verify(&unpinned, downloads(b"source bytes", b"patch bytes"));
        assert_eq!(
            statuses(&report),
            [Status::Passed, Status::Passed, Status::RequiresApproval]
        );

        let mut upstream = policy;
        upstream["patch_origins"][PATCH] = json!({
            "kind": "upstream",
            "repository": UPSTREAM,
            "raw_root": "https://raw.githubusercontent.com/test/demo/",
            "commit": NIXPKGS_COMMIT,
            "path": "patches/fix.patch"
        });
        let upstream_patch = "https://raw.githubusercontent.com/test/demo/0123456789abcdef0123456789abcdef01234567/patches/fix.patch";
        let report = fixture.verify(
            &upstream,
            DownloadsFixture {
                files: BTreeMap::from([
                    (SOURCE, b"source bytes".as_slice()),
                    (upstream_patch, b"patch bytes".as_slice()),
                ]),
                ..DownloadsFixture::default()
            },
        );
        assert_eq!(statuses(&report), [Status::Passed; 3]);
    }

    #[test]
    fn candidate_origin_changes_cannot_authorize_downloads() {
        let fixture = Fixture::new(Some("https://github.com/test/old/"));
        fixture.package_record(UPSTREAM);
        fixture.version(SOURCE, b"source bytes", false);
        fixture.commit();
        let report = fixture.verify(&policy(), DownloadsFixture::default());
        assert_eq!(
            statuses(&report),
            [
                Status::RequiresApproval,
                Status::Incomplete,
                Status::Incomplete
            ]
        );
        assert_eq!(report.findings[0].code, "source_origin_unapproved");

        let mut approved = policy();
        approved["approved_sources"] = json!({"demo": [UPSTREAM]});
        let report = fixture.verify(
            &approved,
            DownloadsFixture {
                files: BTreeMap::from([(SOURCE, b"source bytes".as_slice())]),
                ..DownloadsFixture::default()
            },
        );
        assert_eq!(statuses(&report), [Status::Passed; 3]);
        let report = fixture.verify(
            &approved,
            DownloadsFixture {
                redirects: BTreeMap::from([(SOURCE, "https://github.com/test/demolition/file")]),
                ..DownloadsFixture::default()
            },
        );
        assert_eq!(
            statuses(&report),
            [Status::Passed, Status::Failed, Status::Incomplete]
        );
        assert_eq!(report.findings[0].code, "redirect_outside_origins");
    }
}
